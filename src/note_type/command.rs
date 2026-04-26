use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;

use crate::anki::client::{AnkiClient, anki_client};
use crate::anki::schema::CardTemplate;
use crate::cli::NoteTypeAction;
use crate::note_type::files::{NoteTypeFiles, NoteTypeManifest, TemplateEntry, TemplatePair};
use crate::note_type::paths::slugify;
use crate::note_type::state::{self, SyncState, hash_remote};
use crate::snapshot::store::{
    NoteTypeSnapshot, generate_run_id, generate_timestamp, save_note_type_snapshot,
};

pub fn run(action: NoteTypeAction) -> Result<()> {
    match action {
        NoteTypeAction::Pull { name, force } => pull(&name, force),
        NoteTypeAction::Push {
            name,
            all,
            dry_run,
            no_snapshot,
            force,
        } => {
            if all {
                push_all(dry_run, no_snapshot, force)
            } else {
                // Clap's ArgGroup guarantees one of `name` or `all` is provided.
                push_one(name.as_deref().unwrap(), dry_run, no_snapshot, force)
            }
        }
        NoteTypeAction::Status => status(),
    }
}

fn pull(name: &str, force: bool) -> Result<()> {
    let client = anki_client();
    ensure_model_exists(&client, name)?;

    let css = client.model_styling(name)?;
    let templates = client.model_templates(name)?;

    let root = match NoteTypeFiles::load(name) {
        Ok(existing) => {
            if !force {
                bail!(
                    "'{}' already exists at {}. Re-run with --force to overwrite.",
                    name,
                    existing.root.display()
                );
            }
            existing.root
        }
        Err(_) => NoteTypeFiles::fresh_dir(name)?,
    };

    // Slugify each template name; dedupe collisions by appending -2, -3, ...
    let mut used: HashSet<String> = HashSet::new();
    let mut entries = Vec::with_capacity(templates.len());
    for tname in templates.keys() {
        let base = slugify(tname);
        let mut slug = base.clone();
        let mut n = 2;
        while !used.insert(slug.clone()) {
            slug = format!("{base}-{n}");
            n += 1;
        }
        entries.push(TemplateEntry {
            name: tname.clone(),
            slug,
        });
    }

    let pair_map: IndexMap<String, TemplatePair> = templates
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                TemplatePair {
                    front: v.front.clone(),
                    back: v.back.clone(),
                },
            )
        })
        .collect();

    let files = NoteTypeFiles {
        manifest: NoteTypeManifest {
            name: name.to_string(),
            templates: entries,
        },
        root: root.clone(),
        css,
        templates: pair_map,
    };
    files.write()?;

    let remote_hash = hash_remote(&files.css, &to_wire(&files.templates));
    state::write(
        &root,
        &SyncState {
            last_remote_hash: remote_hash,
        },
    )?;
    state::ensure_gitignored(&root)?;

    println!("\x1b[32m✓\x1b[0m Pulled '{}' to {}", name, root.display());
    println!("  CSS: {} bytes", files.css.len());
    println!("  Templates: {}", files.templates.len());
    Ok(())
}

fn push_one(name: &str, dry_run: bool, no_snapshot: bool, force: bool) -> Result<()> {
    let client = anki_client();
    ensure_model_exists(&client, name)?;
    push_one_with_client(&client, name, dry_run, no_snapshot, force)
}

fn push_one_with_client(
    client: &AnkiClient,
    name: &str,
    dry_run: bool,
    no_snapshot: bool,
    force: bool,
) -> Result<()> {
    let files = NoteTypeFiles::load(name)?;

    let anki_templates = client.model_templates(name)?;
    let mut local_keys: Vec<&String> = files.templates.keys().collect();
    let mut anki_keys: Vec<&String> = anki_templates.keys().collect();
    local_keys.sort();
    anki_keys.sort();
    if local_keys != anki_keys {
        bail!(
            "Template topology mismatch for '{}':\n  local: {:?}\n  Anki:  {:?}\n\
             Adding/removing/renaming card templates via `push` is not supported.\n\
             Make the change in Anki and re-pull.",
            name,
            local_keys,
            anki_keys
        );
    }

    let anki_css = client.model_styling(name)?;
    let current_remote_hash = hash_remote(&anki_css, &anki_templates);
    match state::read(&files.root)? {
        Some(prev) if prev.last_remote_hash != current_remote_hash && !force => {
            bail!(
                "Anki has changed out-of-band for '{}' since the last sync.\n\
                 Run `pull` to take Anki's state, or pass --force to overwrite.",
                name
            );
        }
        None if !force => {
            bail!(
                "No sync state found for '{}'. Run `pull` first, or pass --force to push anyway.",
                name
            );
        }
        _ => {}
    }

    if dry_run {
        println!("\x1b[33mWould push\x1b[0m '{}' to Anki", name);
        println!("  CSS: {} bytes", files.css.len());
        for (tname, tmpl) in &files.templates {
            println!(
                "  Template '{}': front={}b back={}b",
                tname,
                tmpl.front.len(),
                tmpl.back.len()
            );
        }
        return Ok(());
    }

    if !no_snapshot {
        snapshot_note_type(name, &anki_css, &anki_templates)?;
    }

    let wire = to_wire(&files.templates);
    client.update_model_styling(name, &files.css)?;
    if let Err(e) = client.update_model_templates(name, &wire) {
        // Templates failed after styling succeeded. Try to restore the
        // previous styling so Anki isn't left with a half-applied push.
        // Best-effort: report the original error either way.
        if let Err(revert_err) = client.update_model_styling(name, &anki_css) {
            return Err(e).context(format!(
                "templates update failed and styling could not be reverted: {revert_err}"
            ));
        }
        return Err(e).context("templates update failed; styling reverted to previous state");
    }

    let new_hash = hash_remote(&files.css, &wire);
    state::write(
        &files.root,
        &SyncState {
            last_remote_hash: new_hash,
        },
    )?;
    state::ensure_gitignored(&files.root)?;

    println!("\x1b[32m✓\x1b[0m Pushed '{}' to Anki", name);
    Ok(())
}

fn push_all(dry_run: bool, no_snapshot: bool, force: bool) -> Result<()> {
    let dirs = NoteTypeFiles::discover()?;
    if dirs.is_empty() {
        bail!("No note types found under note-types/");
    }

    let client = anki_client();
    let models = client.model_names()?;

    let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
    for dir in &dirs {
        let files = match NoteTypeFiles::load_from_path(dir) {
            Ok(f) => f,
            Err(e) => {
                failures.push((dir.display().to_string(), e));
                continue;
            }
        };
        let name = &files.manifest.name;
        if !models.contains(name) {
            failures.push((
                name.clone(),
                anyhow::anyhow!("not present in Anki (available: {})", models.join(", ")),
            ));
            continue;
        }
        if let Err(e) = push_one_with_client(&client, name, dry_run, no_snapshot, force) {
            failures.push((name.clone(), e));
        }
    }

    if !failures.is_empty() {
        eprintln!("\n\x1b[31m{} note type(s) failed:\x1b[0m", failures.len());
        for (name, err) in &failures {
            eprintln!("  - {name}: {err}");
        }
        bail!("push --all completed with {} failure(s)", failures.len());
    }
    Ok(())
}

fn status() -> Result<()> {
    let dirs = NoteTypeFiles::discover()?;
    if dirs.is_empty() {
        println!("No note types found under note-types/");
        return Ok(());
    }

    let client = anki_client();
    let anki_models = client.model_names()?;
    let mut any_dirty = false;

    for dir in &dirs {
        let files = match NoteTypeFiles::load_from_path(dir) {
            Ok(f) => f,
            Err(e) => {
                println!("\x1b[31m✗\x1b[0m {} — {}", dir.display(), e);
                any_dirty = true;
                continue;
            }
        };
        let name = &files.manifest.name;

        if !anki_models.contains(name) {
            println!("\x1b[31m✗\x1b[0m {} — not present in Anki", name);
            any_dirty = true;
            continue;
        }

        let anki_css = client.model_styling(name)?;
        let anki_templates = client.model_templates(name)?;
        let local_wire = to_wire(&files.templates);

        let local_hash = hash_remote(&files.css, &local_wire);
        let remote_hash = hash_remote(&anki_css, &anki_templates);
        let last = state::read(&files.root)?.map(|s| s.last_remote_hash);

        let local_changed = Some(&local_hash) != last.as_ref();
        let remote_changed = Some(&remote_hash) != last.as_ref();

        match (local_changed, remote_changed) {
            (false, false) => println!("\x1b[32m✓\x1b[0m {} — up to date", name),
            (true, false) => {
                println!("\x1b[33m*\x1b[0m {} — local changes ready to push", name);
                any_dirty = true;
            }
            (false, true) => {
                println!(
                    "\x1b[33m↓\x1b[0m {} — Anki has new changes (run `pull`)",
                    name
                );
                any_dirty = true;
            }
            (true, true) => {
                println!(
                    "\x1b[31m!\x1b[0m {} — diverged (local and Anki both changed)",
                    name
                );
                any_dirty = true;
            }
        }
    }

    if !any_dirty {
        println!("\nAll note types are up to date.");
    }
    Ok(())
}

fn ensure_model_exists(client: &AnkiClient, name: &str) -> Result<()> {
    let models = client.model_names()?;
    if !models.contains(&name.to_string()) {
        bail!(
            "Note type '{}' not found in Anki.\nAvailable: {}",
            name,
            models.join(", ")
        );
    }
    Ok(())
}

fn snapshot_note_type(
    name: &str,
    anki_css: &str,
    anki_templates: &IndexMap<String, CardTemplate>,
) -> Result<()> {
    let snap = NoteTypeSnapshot {
        run_id: generate_run_id(),
        timestamp: generate_timestamp(),
        model_name: name.to_string(),
        css: anki_css.to_string(),
        templates: anki_templates.clone(),
    };
    let path = save_note_type_snapshot(&snap)?;
    println!("  \x1b[2mSnapshot: {}\x1b[0m", path.display());
    Ok(())
}

fn to_wire(templates: &IndexMap<String, TemplatePair>) -> IndexMap<String, CardTemplate> {
    templates
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                CardTemplate {
                    front: v.front.clone(),
                    back: v.back.clone(),
                },
            )
        })
        .collect()
}
