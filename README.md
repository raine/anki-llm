<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/raine/anki-llm/main/meta/logo-dark.svg">
    <img src="https://raw.githubusercontent.com/raine/anki-llm/main/meta/logo.svg" alt="anki-llm logo" width="300">
  </picture>
</p>

<p align="center">
  <strong>Bulk-process, generate, and improve Anki flashcards with LLMs and
  text-to-speech.</strong> anki-llm combines reviewable file workflows, direct
  deck updates, and an interactive generation TUI in one scriptable CLI.
</p>

<p align="center">
  <strong>Docs: <a href="https://anki-llm.raine.dev">anki-llm.raine.dev</a></strong>
</p>

<p align="center">
  <img src="https://anki-llm.raine.dev/anki-llm-generate.webp" alt="anki-llm generation TUI" width="830">
</p>

## What people say

> What's next get AI to answer your flashcards for you?
>
> <cite>grei_earl (Reddit)</cite>

> I love this. The README is extremely detailed and clear, and using
> AnkiConnect to edit decks in-place avoids the usual apkg headaches.
>
> <cite>rahimnathwani (Hacker News)</cite>

> This is cool!
>
> <cite>Hsaeedx (Reddit)</cite>

## Why anki-llm?

- **Process files or decks.** Export notes to reviewable CSV or YAML, process
  them with automatic resume, and import the result, or update a deck directly
  with previews, snapshots, and rollback support.

- **Generate cards interactively.** Create several cards for a term, compare
  them in a terminal UI, edit or regenerate candidates, catch duplicates, and
  import only the cards you choose.

- **Keep workflows in plain files.** Store prompts, note types, and per-project
  settings in a workspace that can be reviewed, versioned, and shared.

- **Add text-to-speech.** Fill audio fields in bulk or synthesize audio while
  generating cards, with voice browsing, caching, and multiple TTS providers.

- **Edit note types with normal tools.** Pull card template HTML and CSS into a
  workspace, edit them in your preferred editor or coding agent, and safely push
  them back to Anki.

- **Bring your preferred model.** Use OpenAI, Gemini, DeepSeek, xAI,
  OpenRouter, Ollama, or any OpenAI-compatible chat completions endpoint.

- **Give agents structured Anki access.** `anki-llm query` exposes AnkiConnect
  through a JSON-oriented CLI for collection inspection and agent-driven
  workflows.

## Quick start

Install with the release script:

```sh
curl -fsSL https://raw.githubusercontent.com/raine/anki-llm/main/scripts/install.sh | bash
```

Or install with Homebrew:

```sh
brew install raine/anki-llm/anki-llm
```

Or install from crates.io:

```sh
cargo install anki-llm
```

Install the
[AnkiConnect add-on](https://ankiweb.net/shared/info/2055492159) and keep Anki
Desktop running for commands that read or change your collection. File-only LLM
processing with `process-file` can run while Anki is closed.

Set an API key for your provider. For example, with OpenAI:

```sh
export OPENAI_API_KEY="your-api-key"
```

Check the resolved model, workspace, credentials, and Anki connection without
changing your collection:

```sh
anki-llm doctor
```

Continue with the
[getting started guide](https://anki-llm.raine.dev/getting-started/) to create a
workspace and run your first processing or generation workflow.

## Documentation

- [What is anki-llm?](https://anki-llm.raine.dev/)
- [Getting started](https://anki-llm.raine.dev/getting-started/)
- [Core concepts](https://anki-llm.raine.dev/concepts/)
- [Process a file](https://anki-llm.raine.dev/process-file/)
- [Process a deck](https://anki-llm.raine.dev/process-deck/)
- [Write prompts](https://anki-llm.raine.dev/prompts/)
- [Use workspaces](https://anki-llm.raine.dev/workspaces/)
- [Generate cards](https://anki-llm.raine.dev/generate/)
- [Configure processing steps](https://anki-llm.raine.dev/processing-steps/)
- [Text-to-speech](https://anki-llm.raine.dev/tts/)
- [Manage note types](https://anki-llm.raine.dev/note-types/)
- [Work with agents](https://anki-llm.raine.dev/agents/)
- [Command reference](https://anki-llm.raine.dev/command-reference/)
- [Prompt reference](https://anki-llm.raine.dev/prompt-reference/)
- [Models and providers](https://anki-llm.raine.dev/models/)
- [TTS providers](https://anki-llm.raine.dev/tts-providers/)
- [Configuration](https://anki-llm.raine.dev/configuration/)
- [AnkiConnect reference](https://anki-llm.raine.dev/ankiconnect/)
- [Troubleshooting](https://anki-llm.raine.dev/troubleshooting/)
- [FAQ](https://anki-llm.raine.dev/faq/)
- [Changelog](https://anki-llm.raine.dev/changelog/)
- [LLM-readable docs](https://anki-llm.raine.dev/llms.txt)

Recipes:

- [Verify translations](https://anki-llm.raine.dev/recipes/translations/)
- [Add key vocabulary](https://anki-llm.raine.dev/recipes/key-vocabulary/)
- [Generate vocabulary cards](https://anki-llm.raine.dev/recipes/vocabulary-cards/)

## Development

Install the local quality tools and run the full read-only check suite:

```sh
just install-quality-tools
just check
```

`just check` writes full command output to `target/check-logs`. Use
`just format` or `just clippy-fix` when source files should be modified.

## License

anki-llm is available under the [MIT License](LICENSE).
