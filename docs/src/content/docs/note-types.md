---
title: "Manage note types"
description: "Pull, edit, compare, and safely push Anki note type templates."
---

`anki-llm note-type` exposes card template HTML and CSS as ordinary local files. You can use your editor, version control, and coding agents, then push the result through AnkiConnect.

Anki must be running with AnkiConnect for pull, status, and push operations.

## Pull a note type

From a [workspace](/workspaces/), run:

```bash
anki-llm note-type pull "Japanese Vocabulary"
```

The note type appears under `note-types/<slug>/`:

```text
note-types/Japanese_Vocabulary/
├── note-type.yaml
├── style.css
├── Recognition.front.html
├── Recognition.back.html
└── .sync-state.json
```

- `note-type.yaml` records the exact Anki model name and canonical template order. Commit it.
- `style.css` contains the note type stylesheet.
- `<template-slug>.front.html` and `.back.html` contain each card template.
- `.sync-state.json` records the last-synced remote hash. anki-llm adds it to the note type directory's `.gitignore`.

Pull refuses to overwrite an existing local note type. Use `pull --force` only when replacing those local files with Anki's current version is intentional.

## Edit locally or with an agent

Open the HTML and CSS in an editor, or give their paths and a concrete design request to a coding agent. For example:

> Redesign `note-types/Japanese_Vocabulary/Recognition.back.html` and `style.css` for a compact reading and meaning layout. Preserve every Anki field reference and card-side behavior.

Review the resulting diff before pushing. Keep `note-type.yaml` aligned with the files produced by `pull`, and do not hand-edit `.sync-state.json`.

A useful version-control cycle is:

```bash
git diff -- note-types/Japanese_Vocabulary
anki-llm note-type status
anki-llm note-type push "Japanese Vocabulary" --dry-run
```

## Understand status

`status` fetches the current Anki templates and compares both sides with the last sync hash:

```bash
anki-llm note-type status
```

| Status | Meaning | Next step |
| --- | --- | --- |
| Up to date | Local files and Anki match the last sync | Edit or continue |
| Local changes ready to push | Only local files changed | Review, dry-run, push |
| Anki has new changes | Only Anki changed | Pull to take Anki's version |
| Diverged | Local files and Anki both changed | Reconcile deliberately before push |

Status also reports invalid local directories and note types missing from Anki.

## Push safely

Preview first:

```bash
anki-llm note-type push "Japanese Vocabulary" --dry-run
```

Then push:

```bash
anki-llm note-type push "Japanese Vocabulary"
```

A normal push applies CSS and existing front and back template bodies. It then updates the local sync hash. If the template update fails after CSS succeeds, anki-llm attempts to restore the previous CSS so Anki is not left with a half-applied update.

Push every local note type with:

```bash
anki-llm note-type push --all
```

Each note type is handled separately, and all failures are reported at the end.

## Snapshots and divergence protection

Before changing Anki, push saves the current remote CSS and templates under:

```text
~/.local/state/anki-llm/note-type-snapshots/<slug>/<run-id>.json
```

`--no-snapshot` disables that backup. Prefer the default unless another durable backup is already in place.

Push refuses these unsafe states:

- Anki changed since the last pull or push.
- No local sync state exists.
- Local and remote template names differ.

Run `pull` to accept the remote version and rebuild local sync state. If both local and remote edits matter, preserve copies and reconcile them before pulling. `--force` explicitly overwrites Anki despite missing or divergent sync state. It does not bypass template-topology checks.

## Limitations

`push` edits CSS and the bodies of existing card templates. Use Anki's GUI to add, remove, rename, or reorder card templates, then run `pull` again. The command does not create or delete note types or change their fields.
