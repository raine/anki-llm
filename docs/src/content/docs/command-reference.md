---
title: "Command reference"
description: "Review every anki-llm command, argument, and option."
---

<!-- Generated from the Clap command tree by `just command-reference`. -->

This reference is generated from the command-line interface definition.

## Command index

- [`anki-llm export`](#command-export)
- [`anki-llm import`](#command-import)
- [`anki-llm process-file`](#command-process-file)
- [`anki-llm process-deck`](#command-process-deck)
- [`anki-llm query`](#command-query)
- [`anki-llm config`](#command-config)
  - [`anki-llm config get`](#command-config-get)
  - [`anki-llm config set`](#command-config-set)
  - [`anki-llm config unset`](#command-config-unset)
  - [`anki-llm config list`](#command-config-list)
  - [`anki-llm config path`](#command-config-path)
- [`anki-llm generate`](#command-generate)
- [`anki-llm generate-init`](#command-generate-init)
- [`anki-llm history`](#command-history)
- [`anki-llm rollback`](#command-rollback)
- [`anki-llm tts`](#command-tts)
- [`anki-llm tts-voices`](#command-tts-voices)
- [`anki-llm update`](#command-update)
- [`anki-llm docs`](#command-docs)
- [`anki-llm workspace`](#command-workspace)
  - [`anki-llm workspace init`](#command-workspace-init)
  - [`anki-llm workspace info`](#command-workspace-info)
- [`anki-llm note-type`](#command-note-type)
  - [`anki-llm note-type pull`](#command-note-type-pull)
  - [`anki-llm note-type push`](#command-note-type-push)
  - [`anki-llm note-type status`](#command-note-type-status)
- [`anki-llm doctor`](#command-doctor)

<a id="command-anki-llm"></a>

## `anki-llm`

Bulk-process Anki flashcards with LLMs

**Usage**

```text
Usage: anki-llm <COMMAND>
```

**Options**

- **`-h, --help`**: Print help
- **`-V, --version`**: Print version

<a id="command-export"></a>

## `anki-llm export`

Export an Anki deck to CSV or YAML

**Usage**

```text
Usage: anki-llm export [OPTIONS] <DECK|--query <QUERY>>
```

**Arguments**

- **`[DECK]`**: Deck name to export

**Options**

- **`-q, --query <QUERY>`**: Anki search query (e.g. "tag:leech", "prop:lapses>3", "deck:Japanese -field:Audio")
- **`-o, --output <OUTPUT>`**: Output file path
- **`-n, --note-type <NOTE_TYPE>`**: Filter by note type (required if deck contains multiple note types)
- **`-h, --help`**: Print help

<a id="command-import"></a>

## `anki-llm import`

Import CSV or YAML file into an Anki deck

**Usage**

```text
Usage: anki-llm import [OPTIONS] --deck <DECK> <INPUT>
```

**Arguments**

- **`<INPUT>`**: Input file path (CSV or YAML)
  - Required: yes

**Options**

- **`-d, --deck <DECK>`**: Target Anki deck name
  - Required: yes
- **`-n, --note-type <NOTE_TYPE>`**: Anki note type name (inferred from deck if not specified)
- **`-k, --key-field <KEY_FIELD>`**: Field name to use for identifying existing notes (auto-detected if not specified)
- **`-h, --help`**: Print help

<a id="command-process-file"></a>

## `anki-llm process-file`

Process notes from a file with AI (supports resume)

**Usage**

```text
Usage: anki-llm process-file [OPTIONS] --prompt <PROMPT> --output <OUTPUT> <INPUT>
```

**Arguments**

- **`<INPUT>`**: Input file path (CSV or YAML)
  - Required: yes

**Options**

- **`-p, --prompt <PROMPT>`**: Path to prompt template file (frontmatter declares output.field or output.fields)
  - Required: yes
- **`-o, --output <OUTPUT>`**: Output file path (CSV or YAML)
  - Required: yes
- **`-m, --model <MODEL>`**: Model name
- **`--api-base-url <API_BASE_URL>`**: Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
- **`--api-key <API_KEY>`**: API key (overrides environment variables)
- **`--reasoning-effort <REASONING_EFFORT>`**: Reasoning effort passed to the model provider
- **`-b, --batch-size <BATCH_SIZE>`**: Number of concurrent requests
  - Default: `5`
- **`-t, --temperature <TEMPERATURE>`**: Sampling temperature (0-2)
- **`--max-tokens <MAX_TOKENS>`**: Maximum tokens per completion
- **`-r, --retries <RETRIES>`**: Number of retries on failure
  - Default: `3`
- **`-f, --force`**: Re-process all rows, ignoring existing output
  - Default: `false`
- **`-d, --dry-run`**: Preview without making API calls
  - Default: `false`
- **`-P, --preview`**: Process a sample of cards with the LLM and show what would change
  - Default: `false`
- **`--preview-count <PREVIEW_COUNT>`**: Number of cards to process in preview mode (default: 3)
  - Default: `3`
- **`--limit <LIMIT>`**: Limit the number of rows to process
- **`--log <LOG>`**: Append raw LLM prompts and responses to a log file (relative path)
- **`--very-verbose`**: Print raw LLM prompts and responses to stderr
  - Default: `false`
- **`-h, --help`**: Print help

<a id="command-process-deck"></a>

## `anki-llm process-deck`

Process notes directly from an Anki deck

**Usage**

```text
Usage: anki-llm process-deck [OPTIONS] --prompt <PROMPT> <DECK|--query <QUERY>>
```

**Arguments**

- **`[DECK]`**: Deck name to process

**Options**

- **`-q, --query <QUERY>`**: Anki search query (e.g. "tag:leech", "prop:lapses>3", "deck:Japanese prop:lapses>5")
- **`-p, --prompt <PROMPT>`**: Path to prompt template file (frontmatter declares output.field or output.fields)
  - Required: yes
- **`-n, --note-type <NOTE_TYPE>`**: Filter by note type (required if deck contains multiple note types)
- **`-m, --model <MODEL>`**: Model name
- **`--api-base-url <API_BASE_URL>`**: Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
- **`--api-key <API_KEY>`**: API key (overrides environment variables)
- **`--reasoning-effort <REASONING_EFFORT>`**: Reasoning effort passed to the model provider
- **`-b, --batch-size <BATCH_SIZE>`**: Number of concurrent requests
  - Default: `5`
- **`-t, --temperature <TEMPERATURE>`**: Sampling temperature (0-2)
- **`--max-tokens <MAX_TOKENS>`**: Maximum tokens per completion
- **`-r, --retries <RETRIES>`**: Number of retries on failure
  - Default: `3`
- **`-d, --dry-run`**: Preview without making API calls
  - Default: `false`
- **`-P, --preview`**: Process a sample of cards with the LLM and show what would change
  - Default: `false`
- **`--preview-count <PREVIEW_COUNT>`**: Number of cards to process in preview mode (default: 3)
  - Default: `3`
- **`--limit <LIMIT>`**: Limit the number of notes to process
- **`-f, --force`**: Re-process notes even if the target field already has content
  - Default: `false`
- **`--log <LOG>`**: Append raw LLM prompts and responses to a log file (relative path)
- **`--very-verbose`**: Print raw LLM prompts and responses to stderr
  - Default: `false`
- **`-h, --help`**: Print help

<a id="command-query"></a>

## `anki-llm query`

Query AnkiConnect API

**Usage**

```text
Usage: anki-llm query <ACTION> [PARAMS]
```

**Arguments**

- **`<ACTION>`**: AnkiConnect action name
  - Required: yes
- **`[PARAMS]`**: JSON parameters

**Options**

- **`-h, --help`**: Print help

<a id="command-config"></a>

## `anki-llm config`

Manage persistent configuration

**Usage**

```text
Usage: anki-llm config <COMMAND>
```

**Options**

- **`-h, --help`**: Print help

<a id="command-config-get"></a>

### `anki-llm config get`

Get a configuration value

**Usage**

```text
Usage: anki-llm config get <KEY>
```

**Arguments**

- **`<KEY>`**: Config key
  - Required: yes

**Options**

- **`-h, --help`**: Print help

<a id="command-config-set"></a>

### `anki-llm config set`

Set a configuration value

**Usage**

```text
Usage: anki-llm config set <KEY> <VALUE>
```

**Arguments**

- **`<KEY>`**: Config key
  - Required: yes
- **`<VALUE>`**: Config value
  - Required: yes

**Options**

- **`-h, --help`**: Print help

<a id="command-config-unset"></a>

### `anki-llm config unset`

Unset a configuration value

**Usage**

```text
Usage: anki-llm config unset <KEY>
```

**Arguments**

- **`<KEY>`**: Config key
  - Required: yes

**Options**

- **`-h, --help`**: Print help

<a id="command-config-list"></a>

### `anki-llm config list`

List all configuration settings

**Usage**

```text
Usage: anki-llm config list
```

**Options**

- **`-h, --help`**: Print help

<a id="command-config-path"></a>

### `anki-llm config path`

Show the config file path

**Usage**

```text
Usage: anki-llm config path
```

**Options**

- **`-h, --help`**: Print help

<a id="command-generate"></a>

## `anki-llm generate`

Generate Anki cards using an LLM

**Usage**

```text
Usage: anki-llm generate [OPTIONS] [TERM]
```

**Arguments**

- **`[TERM]`**: Term to generate cards for (omit to enter interactively in TUI)

**Options**

- **`-p, --prompt <PROMPT>`**: Path to prompt template file with frontmatter (auto-resolved from the workspace's prompts/ if omitted)
- **`-c, --count <COUNT>`**: Number of card examples to generate
  - Default: `3`
- **`-m, --model <MODEL>`**: Model name
- **`--api-base-url <API_BASE_URL>`**: Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
- **`--api-key <API_KEY>`**: API key (overrides environment variables)
- **`--reasoning-effort <REASONING_EFFORT>`**: Reasoning effort passed to the model provider
- **`-d, --dry-run`**: Preview without importing to Anki
  - Default: `false`
- **`-r, --retries <RETRIES>`**: Number of retries for failed requests
  - Default: `3`
- **`--max-tokens <MAX_TOKENS>`**: Maximum tokens per response
- **`-t, --temperature <TEMPERATURE>`**: LLM temperature (0-2)
- **`-o, --output <OUTPUT>`**: Export cards to a file instead of importing to Anki
- **`--copy`**: Copy prompt to clipboard for manual LLM mode
  - Default: `false`
- **`--log <LOG>`**: Append raw LLM prompts and responses to a log file (relative path)
- **`--very-verbose`**: Print raw LLM prompts and responses to stderr
  - Default: `false`
- **`-h, --help`**: Print help

<a id="command-generate-init"></a>

## `anki-llm generate-init`

Create a prompt template by querying your Anki collection

**Usage**

```text
Usage: anki-llm generate-init [OPTIONS] [OUTPUT]
```

**Arguments**

- **`[OUTPUT]`**: Output file path

**Options**

- **`-m, --model <MODEL>`**: Model name
- **`--api-base-url <API_BASE_URL>`**: Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
- **`--api-key <API_KEY>`**: API key (overrides environment variables)
- **`--reasoning-effort <REASONING_EFFORT>`**: Reasoning effort passed to the model provider
- **`-t, --temperature <TEMPERATURE>`**: LLM temperature (0-2)
- **`--copy`**: Copy prompt to clipboard for manual LLM mode
  - Default: `false`
- **`-h, --help`**: Print help

<a id="command-history"></a>

## `anki-llm history`

List past process-deck runs with snapshot data

**Usage**

```text
Usage: anki-llm history
```

**Options**

- **`-h, --help`**: Print help

<a id="command-rollback"></a>

## `anki-llm rollback`

Rollback a previous process-deck run

**Usage**

```text
Usage: anki-llm rollback [OPTIONS] <RUN_ID>
```

**Arguments**

- **`<RUN_ID>`**: Run ID to rollback (from `history` command)
  - Required: yes

**Options**

- **`--force`**: Force rollback even if notes were modified after the run
  - Default: `false`
- **`-d, --dry-run`**: Preview without making changes
  - Default: `false`
- **`-h, --help`**: Print help

<a id="command-tts"></a>

## `anki-llm tts`

Generate text-to-speech audio for notes and upload to Anki's media store

**Usage**

```text
Usage: anki-llm tts [OPTIONS] [DECK]
```

**Arguments**

- **`[DECK]`**: Deck name to process (defaults to deck from --prompt frontmatter)

**Options**

- **`-q, --query <QUERY>`**: Anki search query (e.g. "tag:leech", "deck:Japanese -Audio:")
- **`--prompt <PROMPT>`**: Path to a generate prompt YAML; reads its `tts:` block instead of taking deck-design flags on the CLI
- **`--field <FIELD>`**: Target field to write [sound:...] into (flag mode; required)
- **`-p, --template <TEMPLATE>`**: Path to prompt template file (flag mode)
- **`--text-field <TEXT_FIELD>`**: Source field name (flag mode; alternative to --template)
- **`--provider <PROVIDER>`**: TTS provider identifier (flag mode; defaults to "openai")
- **`--voice <VOICE>`**: Voice name (flag mode; provider-specific, e.g. alloy)
- **`--tts-model <TTS_MODEL>`**: TTS backing model (flag mode; e.g. gpt-4o-mini-tts)
- **`--format <FORMAT>`**: Output audio format (flag mode; defaults to "mp3")
- **`--speed <SPEED>`**: Playback speed (flag mode; 1.0 = normal)
- **`--api-base-url <API_BASE_URL>`**: API base URL override (OpenAI or OpenAI-compatible providers)
- **`--api-key <API_KEY>`**: API key override. Used as the OpenAI bearer token or the Azure subscription key depending on the active provider
- **`--azure-region <AZURE_REGION>`**: Azure region override (flag mode; provider must be 'azure')
- **`--aws-region <AWS_REGION>`**: AWS region override for Amazon Polly (flag mode; provider must be 'amazon')
- **`--aws-access-key-id <AWS_ACCESS_KEY_ID>`**: AWS access key id for Amazon Polly (flag mode)
- **`--aws-secret-access-key <AWS_SECRET_ACCESS_KEY>`**: AWS secret access key for Amazon Polly (flag mode)
- **`-n, --note-type <NOTE_TYPE>`**: Filter by note type (flag mode; rejected in --prompt mode)
- **`-b, --batch-size <BATCH_SIZE>`**: Number of concurrent TTS requests
  - Default: `5`
- **`-r, --retries <RETRIES>`**: Number of retries on transient failures
  - Default: `3`
- **`-f, --force`**: Regenerate audio even for notes whose target field already contains a sound tag
  - Default: `false`
- **`-d, --dry-run`**: Preview without making API calls or mutating Anki
  - Default: `false`
- **`--limit <LIMIT>`**: Limit the number of notes to process
- **`-h, --help`**: Print help

<a id="command-tts-voices"></a>

## `anki-llm tts-voices`

Browse and audition TTS voices across OpenAI / Azure / Google / Polly

**Usage**

```text
Usage: anki-llm tts-voices [OPTIONS]
```

**Options**

- **`--lang <LANG>`**: Pre-filter by language code prefix, e.g. "ja" or "en-US"
- **`--provider <PROVIDER>`**: Pre-filter by provider id (openai, azure, google, amazon, edge)
- **`-q, --query <QUERY>`**: Initial text query for the omni-search field
- **`-h, --help`**: Print help

<a id="command-update"></a>

## `anki-llm update`

Update anki-llm to the latest version

**Usage**

```text
Usage: anki-llm update
```

**Options**

- **`-h, --help`**: Print help

<a id="command-docs"></a>

## `anki-llm docs`

Show the complete documentation bundle offline

**Usage**

```text
Usage: anki-llm docs
```

**Options**

- **`-h, --help`**: Print help

<a id="command-workspace"></a>

## `anki-llm workspace`

Manage anki-llm workspaces

**Usage**

```text
Usage: anki-llm workspace <COMMAND>
```

**Options**

- **`-h, --help`**: Print help

<a id="command-workspace-init"></a>

### `anki-llm workspace init`

Initialize a new workspace in the current or specified directory

**Usage**

```text
Usage: anki-llm workspace init [DIR]
```

**Arguments**

- **`[DIR]`**: Directory to initialize (defaults to current directory)

**Options**

- **`-h, --help`**: Print help

<a id="command-workspace-info"></a>

### `anki-llm workspace info`

Show information about the current workspace

**Usage**

```text
Usage: anki-llm workspace info
```

**Options**

- **`-h, --help`**: Print help

<a id="command-note-type"></a>

## `anki-llm note-type`

Manage Anki note type templates and CSS

**Usage**

```text
Usage: anki-llm note-type <COMMAND>
```

**Options**

- **`-h, --help`**: Print help

<a id="command-note-type-pull"></a>

### `anki-llm note-type pull`

Pull note type templates and CSS from Anki into local files

**Usage**

```text
Usage: anki-llm note-type pull [OPTIONS] <NAME>
```

**Arguments**

- **`<NAME>`**: Note type name (as shown in Anki)
  - Required: yes

**Options**

- **`--force`**: Overwrite existing local files without prompting
  - Default: `false`
- **`-h, --help`**: Print help

<a id="command-note-type-push"></a>

### `anki-llm note-type push`

Push local templates and CSS to Anki

**Usage**

```text
Usage: anki-llm note-type push [OPTIONS] <NAME|--all>
```

**Arguments**

- **`[NAME]`**: Note type name (as shown in Anki)

**Options**

- **`--all`**: Push all note types in the workspace
  - Default: `false`
- **`-d, --dry-run`**: Preview changes without modifying Anki
  - Default: `false`
- **`--no-snapshot`**: Skip snapshot before push
  - Default: `false`
- **`--force`**: Push even if Anki has diverged from the last known remote state
  - Default: `false`
- **`-h, --help`**: Print help

<a id="command-note-type-status"></a>

### `anki-llm note-type status`

Show which note types have un-pushed changes

**Usage**

```text
Usage: anki-llm note-type status
```

**Options**

- **`-h, --help`**: Print help

<a id="command-doctor"></a>

## `anki-llm doctor`

Diagnose configuration and provider connectivity

**Usage**

```text
Usage: anki-llm doctor [OPTIONS]
```

**Options**

- **`--check`**: Verify provider authentication and AnkiConnect reachability over the network
  - Default: `false`
- **`-h, --help`**: Print help
