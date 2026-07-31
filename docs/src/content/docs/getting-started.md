---
title: "Getting started"
description: "Install anki-llm, connect it to Anki, and run a safe first workflow."
---

This guide installs `anki-llm`, connects it to an OpenAI model, creates a
workspace, and generates one candidate card without changing your collection.

## 1. Install `anki-llm`

Choose one installation path.

### Install script

```sh
curl -fsSL https://raw.githubusercontent.com/raine/anki-llm/main/scripts/install.sh | bash
```

### Homebrew on macOS or Linux

```sh
brew install raine/anki-llm/anki-llm
```

### Cargo

```sh
cargo install anki-llm
```

Confirm that the command is available:

```sh
anki-llm --version
```

## 2. Connect Anki Desktop

Install the
[AnkiConnect add-on](https://ankiweb.net/shared/info/2055492159) in Anki
Desktop, then restart Anki. Keep Anki running while a command reads or modifies
your collection.

`anki-llm` uses AnkiConnect's local HTTP API to discover decks and note types,
export and update notes, add generated cards, upload media, and manage card
templates. `process-file` can run with Anki closed once you have exported the
input file.

Test the connection:

```sh
anki-llm query version
```

A JSON version number means the connection works. See [AnkiConnect](/ankiconnect/)
for connection settings and troubleshooting.

:::caution
Commands that access your collection use the Anki profile that is open in Anki
Desktop. Confirm the active profile before a bulk operation.
:::

## 3. Configure one LLM provider

Create an [OpenAI API key](https://platform.openai.com/api-keys), then expose it
to `anki-llm` in your shell:

```sh
export OPENAI_API_KEY="your-api-key-here"
```

OpenAI models are detected from their names, so this provider needs no base URL.
Store a default model if you do not want to pass `--model` on each command:

```sh
anki-llm config set model gpt-5-mini
```

Run the environment diagnostic:

```sh
anki-llm doctor
```

`doctor` reports the detected key, resolved model, active workspace, and
AnkiConnect status. Provider keys are secrets. Keep them in environment
variables or a secret manager rather than prompt files or version control.

For Gemini, OpenRouter, Ollama, and other compatible endpoints, see
[Models](/models/) and [Configuration](/configuration/).

## 4. Create a workspace

A [workspace](/workspaces/) keeps prompts, note-type files, and model settings
together. Create one in a directory you can put under version control:

```sh
mkdir my-anki-workspace
cd my-anki-workspace
anki-llm workspace init
```

This creates:

```text
my-anki-workspace/
├── anki-llm.yaml
└── prompts/
```

Check what `anki-llm` resolved:

```sh
anki-llm workspace info
```

## 5. Create a prompt for your deck

Run the prompt wizard:

```sh
anki-llm generate-init
```

Choose a deck and note type. The wizard samples your existing cards, asks the
LLM to infer their shape and style, and saves a generated prompt under
`prompts/`. Review that file before using it. Its frontmatter identifies the
Anki deck, note type, and field mapping, while its body controls generated
content.

## 6. Run one safe candidate

Generate one card without importing it:

```sh
anki-llm generate "a term from your subject" --count 1 --dry-run
```

This command calls the model and prints the generated card, but `--dry-run`
prevents the normal selection and import flow. Inspect the content, field
mapping, and formatting. Edit the prompt and repeat until the result fits your
deck.

:::note[Cost of this step]
The wizard and generation request call the configured provider. A single
candidate keeps the trial small, and `anki-llm` reports token use and an
estimated cost when pricing is known. See [Cost awareness](/concepts/#cost-awareness).
:::

When the output is ready, remove `--dry-run` to open the selection interface:

```sh
anki-llm generate "a term from your subject" --count 3
```

Review each candidate and import only the cards you select.

## Next steps

- Learn the safety model and shared vocabulary in [Concepts](/concepts/).
- Ask a coding agent to coordinate the workflow in [Work with agents](/agents/).
- Transform exported notes with [Process a file](/process-file/).
- Update notes in place with [Process a deck](/process-deck/).
- Refine generation behavior in [Write prompts](/prompts/) and
  [Prompt reference](/prompt-reference/).
- Keep project-specific defaults in [Use workspaces](/workspaces/).
