---
title: "anki-llm"
description: "Bulk-process, generate, and maintain Anki flashcards with LLMs and text-to-speech."
template: splash
hero:
  tagline: "A CLI and terminal UI for repeatable, reviewable AI workflows over your Anki collection."
  image:
    html: '<img src="/anki-llm-generate.webp" alt="anki-llm generating Japanese vocabulary cards in a terminal interface" width="830">'
  actions:
    - text: "Get started"
      link: "/getting-started/"
      icon: "right-arrow"
    - text: "Work with an agent"
      link: "/agents/"
      variant: "secondary"
---

`anki-llm` connects Anki Desktop to OpenAI-compatible language models. Use it to
transform many existing notes, generate candidate cards for review, add audio,
or maintain card templates as files. It is designed for collection-scale work
where repeatability and inspection matter more than one-off chat interactions.

## What can you do with it?

- **Improve existing notes:** verify translations, add grammar explanations,
  create hints, or fill structured fields across a deck.
- **Generate cards:** request several contextual examples for a term, inspect
  them in the terminal UI, and import only the cards you select.
- **Add speech:** fill audio fields with text-to-speech from OpenAI, Azure,
  Google Cloud, Amazon Polly, or Microsoft Edge.
- **Maintain note types:** pull card template HTML and CSS into ordinary files,
  edit them with your normal tools, and safely push them back to Anki.
- **Automate collection tasks:** expose AnkiConnect as clean JSON through
  `anki-llm query`, including workflows driven by coding agents.

## Choose a workflow

| Goal | Start with | Why |
| --- | --- | --- |
| Review changes in a file before touching Anki | [Process a file](/process-file/) | Export, transform, diff, then import. Large jobs can resume. |
| Update matching notes directly | [Process a deck](/process-deck/) | One command, small previews, automatic snapshots, and rollback. |
| Create cards from a word or concept | [Generate cards](/generate/) | Review and edit candidate cards in an interactive terminal UI. |
| Add pronunciation audio | [Text-to-speech](/tts/) | Fill audio fields while preserving existing audio by default. |
| Redesign card templates with an editor or agent | [Manage note types](/note-types/) | Keep HTML and CSS in files with diffs and version control. |
| Describe an outcome instead of assembling commands | [Work with agents](/agents/) | Let an agent inspect the collection, prepare files, and begin with a safe sample. |

:::tip[Unsure where to begin?]
Use the [file-based workflow](/process-file/) when you want a reviewable staging
file or expect interruptions. Use [direct processing](/process-deck/) for a
shorter workflow with automatic snapshots and rollback.
:::

## Built for controlled bulk work

`anki-llm` combines:

- CSV and YAML export and import
- custom prompt files with explicit output fields
- direct-to-Anki and file-based batch processing
- concurrency, retries, incremental progress, and file-mode resume
- preview, dry-run, and limit controls
- direct-processing snapshots, history, and conflict-aware rollback
- model selection for OpenAI, Gemini, DeepSeek, xAI, OpenRouter, Ollama, and
  other OpenAI-compatible endpoints
- token accounting and estimated costs for known models
- a generation TUI with duplicate detection, editing, regeneration, and model
  switching
- optional manual copy mode for browser-based LLMs
- integrated text-to-speech and note-type file workflows

## Why files and explicit review steps?

An Anki collection is valuable, stateful data. Prompt files make transformations
repeatable and reviewable. Small previews expose prompt mistakes before they
reach a whole deck. Exported files create a diffable boundary, while direct runs
create rollback snapshots. These choices let you use probabilistic models
without treating their output as automatically correct.

Ready to try it? [Install `anki-llm` and run a safe first generation](/getting-started/).
