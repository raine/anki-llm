---
title: "Work with agents"
description: "Let a coding agent inspect Anki, prepare repeatable workflows, and begin with safe previews."
---

A coding agent can coordinate `anki-llm` from a natural-language goal. Instead
of translating that goal into every CLI flag yourself, ask the agent to inspect
the collection, write a prompt, run a small preview, and show you the result
before scaling.

For example:

> Use `anki-llm` to add concise grammar breakdowns to my Japanese cards. Inspect
> the collection first, choose an existing empty field, and start with five
> cards. Show me the proposed prompt and preview before changing any notes.

The CLI remains the execution and safety layer. The agent uses its JSON output,
files, diffs, dry runs, previews, snapshots, and rollback commands.

## Give the agent a safe workflow

A useful request states:

1. the desired outcome
2. the deck or search scope
3. fields that must be preserved
4. the size of the first sample
5. the review point that requires your approval

Ask the agent to follow this sequence:

1. Run `anki-llm doctor` and inspect the active workspace.
2. Query deck, note type, and field names instead of guessing them.
3. Write the prompt and any query parameters to files in the workspace.
4. Run a dry run on a small selection.
5. Run an LLM-backed preview and present the diff.
6. Stop before the first applied mutation unless you already authorized it.
7. Scale only after the sample is accepted.
8. Report the direct-run snapshot ID or preserve the file-based inputs and
   outputs.

:::caution
An agent can issue commands quickly, including `--force`. State the approval
boundary explicitly and require small previews for collection changes. Keep Anki
backups even when snapshots are available.
:::

See [Concepts](/concepts/#preview-dry-run-and-limit) for the exact distinction
between dry-run, preview, and limit.

## Inspect Anki with `anki-llm query`

`anki-llm query <action> [params]` calls any supported AnkiConnect action and
prints clean JSON. This gives an agent a scriptable way to discover your actual
collection before it prepares a transformation.

```sh
# List decks
anki-llm query deckNames

# List note types
anki-llm query modelNames

# Find notes without loading every note body
anki-llm query findNotes '{"query":"deck:Japanese tag:leech"}'

# Read selected notes
anki-llm query notesInfo '{"notes":[1498938915662]}'

# Read deck statistics
anki-llm query getDeckStats '{"decks":["Japanese"]}'
```

Parameters are a JSON object passed as one shell argument. Let the agent build
that JSON rather than manually escaping a long request.

### Give the agent the action reference

Run this once when an agent needs to choose among AnkiConnect actions:

```sh
anki-llm query docs
```

`docs` and `help` return the complete bundled AnkiConnect action documentation.
The same reference is available as
[`ANKI_CONNECT.md`](https://github.com/raine/anki-llm/blob/main/ANKI_CONNECT.md).

## Keep reusable queries as documents

A one-line query can stay in shell history. For analysis, migrations, or
multi-step work, ask the agent to create a short query document in your
workspace. Record:

- the natural-language goal
- the exact Anki search string
- action names and JSON parameters
- output filenames
- assumptions about decks, note types, and fields
- the preview and approval boundary

This creates a reviewable specification for what the agent selected. It also
makes a future rerun safer than reconstructing intent from terminal history.
Query outputs can contain private study material, so decide whether they belong
in version control.

## Edit note types through files

Agents are especially effective at card template work because `anki-llm` can
represent a note type as ordinary HTML, CSS, and YAML files.

```sh
anki-llm note-type pull "Japanese Vocabulary"
anki-llm note-type status
```

Ask the agent to edit files under `note-types/<slug>/`, then inspect the diff.
Preview the push without changing Anki:

```sh
anki-llm note-type push "Japanese Vocabulary" --dry-run
```

After approval, apply it:

```sh
anki-llm note-type push "Japanese Vocabulary"
```

A push snapshots the current Anki templates and refuses to overwrite remote
changes that diverged from the last sync unless `--force` is supplied. Adding,
removing, renaming, or reordering card templates remains an Anki GUI task. See
[Manage note types](/note-types/) for layout and safety details.

## Give agents LLM-readable documentation

The documentation site publishes text endpoints intended for language models:

- [`/llms.txt`](/llms.txt) is the entry point and links the available sets.
- [`/llms-small.txt`](/llms-small.txt) contains an abridged documentation set.
- [`/llms-full.txt`](/llms-full.txt) contains the complete documentation set.

Give an agent `https://anki-llm.raine.dev/llms.txt` when it needs product
concepts, workflows, prompt formats, and command behavior. The generated text
comes from the same source as this site.

## Product docs and action docs serve different jobs

Use the **product documentation** on this site to understand `anki-llm`:
workflows, CLI commands, prompt formats, workspaces, safety behavior, and model
configuration.

Use the **AnkiConnect action documentation** from `anki-llm query docs` or
[`ANKI_CONNECT.md`](https://github.com/raine/anki-llm/blob/main/ANKI_CONNECT.md)
to choose low-level actions and construct their JSON parameters.

An agent usually needs both. Product docs answer "How should this operation be
performed safely with `anki-llm`?" Action docs answer "Which AnkiConnect action
and parameter shape exposes this collection operation?"

## Prompt examples for your agent

### Enrich existing notes

> Inspect my `Japanese` deck and its note types with `anki-llm query`. Propose a
> prompt that fills an existing empty `Grammar` field without changing any other
> fields. Dry-run five notes, then use `--preview` for three model responses.
> Show me the prompt and preview. Do not apply the full run.

### Build a reviewable file pipeline

> Export notes matching `deck:Spanish tag:needs-review` to YAML. Create a prompt
> that verifies the `English` translation from the `Spanish` field. Process ten
> rows first and preserve the original export. Show me a diff of the output and
> estimate full-run cost. Do not import anything yet.

### Analyze a collection

> Use `anki-llm query docs` to find the right actions, then report note counts by
> deck and note type. Save the query goal, exact actions, and parameters to a
> workspace document. Do not modify Anki.

### Redesign a card template

> Pull the `Japanese Vocabulary` note type with `anki-llm note-type`. Improve the
> back template's reading hierarchy and mobile layout while preserving every
> field reference. Show the file diff and a dry-run push. Do not push until I
> approve it.

### Generate cards

> Inspect my vocabulary deck, run `generate-init` in the active workspace, and
> review the generated prompt for the correct field mapping. Generate one card
> for `会議` with `--dry-run`. Show me the card and estimated cost before opening
> the import flow.

For complete prompt files you can adapt, point the agent at the
[`examples/` directory](https://github.com/raine/anki-llm/tree/main/examples).
