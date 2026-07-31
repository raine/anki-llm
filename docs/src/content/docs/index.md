---
title: What is anki-llm?
description: Bulk-process, generate, and maintain Anki flashcards with LLMs and text-to-speech.
---

`anki-llm` is a CLI and terminal UI for repeatable, reviewable AI workflows over an Anki collection. It connects Anki Desktop to OpenAI-compatible language models to transform existing notes, generate candidate cards, add audio, and maintain card templates as files.

![anki-llm generating Japanese vocabulary cards in a terminal interface](/anki-llm-generate.webp)

## Why anki-llm?

- Improve existing notes by verifying translations, adding grammar explanations, creating hints, or filling structured fields across a deck.
- Generate several contextual cards for a term, inspect and edit them in the terminal UI, and import only the cards you select.
- Add speech with OpenAI, Azure, Google Cloud, Amazon Polly, or Microsoft Edge text-to-speech.
- Pull note type HTML and CSS into ordinary files, edit them with normal tools, and safely push them back to Anki.
- Use OpenAI, Gemini, DeepSeek, xAI, OpenRouter, Ollama, or another OpenAI-compatible endpoint, with model selection, token accounting, and estimated costs for known models.
- Give coding agents structured access to collection data through the JSON-oriented `anki-llm query` command.

## Choose a workflow

- [Process a file](/process-file/) to export, transform, diff, and import a reviewable CSV or YAML file. File processing supports incremental output and resume for interrupted jobs.
- [Process a deck](/process-deck/) to update matching notes directly with a small preview, automatic snapshots, history, and conflict-aware rollback.
- [Generate cards](/generate/) to review, edit, regenerate, and deduplicate candidate cards in an interactive terminal UI before import.
- [Use text-to-speech](/tts/) to fill audio fields while preserving existing audio by default.
- [Manage note types](/note-types/) to keep card template HTML and CSS in files that work with diffs, version control, editors, and coding agents.
- [Work with agents](/agents/) to describe an outcome while an agent inspects the collection, prepares files, and begins with a safe sample.

## Design principles

An Anki collection is valuable, stateful data, while language model output is probabilistic. Prompt files make transformations repeatable and explicit output fields keep updates constrained. Preview, dry-run, and limit controls expose mistakes before a whole deck is changed. Exported files provide a diffable review boundary, while direct processing creates rollback snapshots.

Batch workflows support concurrency, retries, incremental progress, and file-mode resume. They can run against local or hosted OpenAI-compatible endpoints. Manual copy mode also supports browser-based language models when an API workflow is not appropriate.

Generation keeps acceptance decisions with the user. The terminal UI supports duplicate detection, editing, regeneration, model switching, and optional text-to-speech before selected cards enter Anki.
