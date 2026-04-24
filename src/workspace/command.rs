use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cli::WorkspaceAction;
use crate::config::store::read_config;
use crate::workspace::context::Workspace;
use crate::workspace::manifest::{MANIFEST_FILE_NAME, WorkspaceManifest, write_manifest};

pub fn run(action: WorkspaceAction) -> Result<()> {
    match action {
        WorkspaceAction::Init { dir } => init(dir),
        WorkspaceAction::Info => info(),
    }
}

fn init(dir: Option<PathBuf>) -> Result<()> {
    let root = dir.unwrap_or_else(|| std::env::current_dir().unwrap());
    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;

    let manifest_path = root.join(MANIFEST_FILE_NAME);
    let wrote_manifest = if !manifest_path.exists() {
        let manifest = WorkspaceManifest::default();
        write_manifest(&manifest_path, &manifest)?;
        true
    } else {
        false
    };

    let prompts_dir = root.join("prompts");
    let created_prompts = if !prompts_dir.exists() {
        fs::create_dir_all(&prompts_dir)
            .with_context(|| format!("failed to create {}", prompts_dir.display()))?;
        true
    } else {
        false
    };

    if wrote_manifest || created_prompts {
        println!(
            "\x1b[32m\u{2713}\x1b[0m Initialized workspace at {}",
            root.display()
        );
        if wrote_manifest {
            println!("  \x1b[2m{}\x1b[0m", manifest_path.display());
        }
        if created_prompts {
            println!("  \x1b[2m{}/\x1b[0m", prompts_dir.display());
        }
    } else {
        println!(
            "\x1b[32m\u{2713}\x1b[0m Workspace already exists at {}",
            root.display()
        );
    }

    Ok(())
}

fn info() -> Result<()> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;

    if let Some(workspace) = Workspace::in_dir(&cwd) {
        println!("\x1b[32m\u{2713}\x1b[0m Workspace found");
        println!("  Root: {}", workspace.root.display());
        if let Some(ref model) = workspace.manifest.default_model {
            println!("  Default model: {}", model);
        }
        let prompts = workspace.prompts_dir();
        println!("  Prompts: {}/", prompts.display());
        if workspace.note_types_dir().exists() {
            println!("  Note-types: {}/", workspace.note_types_dir().display());
        }
    } else {
        println!("\x1b[33mNo workspace found\x1b[0m in current directory.");
        println!();
        println!(
            "A workspace is just a directory with a prompts/ folder (and optionally anki-llm.yaml)."
        );
        println!();
        println!("To create one:");
        println!("  anki-llm workspace init");

        // Show fallback config, if any.
        if let Ok(config) = read_config()
            && let Some(dir) = config.prompts_dir
        {
            println!();
            println!("Falling back to config prompts_dir:");
            println!("  {}", dir.display());
        }
    }

    Ok(())
}
