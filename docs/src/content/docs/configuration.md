---
title: "Configuration"
description: "Look up global settings, environment variables, workspace precedence, and local state paths."
---

## Global configuration

Global settings live at:

```text
~/.config/anki-llm/config.json
```

Manage them through the CLI so values are validated and written atomically:

```bash
anki-llm config set model gpt-5-mini
anki-llm config get model
anki-llm config list
anki-llm config unset model
```

### Global keys

| Key | Value | Purpose |
| --- | --- | --- |
| `model` | string | Default LLM model |
| `api_base_url` | URL | Default OpenAI-compatible endpoint |
| `reasoning_effort` | non-empty string | Default reasoning effort |
| `gemini_thinking_enabled` | boolean | Show and request supported Gemini thinking; defaults to true |
| `nerd_font` | boolean | Use Nerd Font symbols in terminal interfaces |
| `default_workspace` | path | Workspace fallback outside a workspace directory |
| `anki_connect_url` | URL | AnkiConnect endpoint override |
| `tts_provider` | provider ID | Default TTS provider |
| `tts_voice` | voice ID | Default TTS voice |
| `tts_model` | string | OpenAI TTS model or Amazon Polly engine |
| `tts_format` | string | Default audio format |
| `azure_tts_key` | secret | Azure Speech subscription key |
| `azure_tts_region` | region | Azure Speech region |
| `google_tts_key` | secret | Google Cloud TTS API key |
| `aws_tts_access_key_id` | secret | AWS access key ID for Polly |
| `aws_tts_secret_access_key` | secret | AWS secret access key for Polly |
| `aws_tts_region` | region | AWS region for Polly |

Global config has no LLM API key key. Prefer environment variables for all secrets, including TTS credentials, so they do not reside in `config.json`.

## Environment variables

### LLM

| Variable                    | Purpose                                           |
| --------------------------- | ------------------------------------------------- |
| `ANKI_LLM_API_BASE_URL`     | Generic compatible API endpoint                   |
| `ANKI_LLM_API_KEY`          | Generic key, including OpenRouter and custom APIs |
| `ANKI_LLM_REASONING_EFFORT` | Reasoning effort override                         |
| `OPENAI_API_KEY`            | Auto-detected OpenAI key and OpenAI TTS fallback  |
| `GEMINI_API_KEY`            | Auto-detected Google Gemini key                   |
| `DEEPSEEK_API_KEY`          | Auto-detected DeepSeek key                        |
| `XAI_API_KEY`               | Auto-detected xAI key                             |

### TTS

| Variable                | Purpose                                           |
| ----------------------- | ------------------------------------------------- |
| `AZURE_TTS_KEY`         | Azure Speech subscription key                     |
| `AZURE_TTS_REGION`      | Azure Speech region                               |
| `GOOGLE_TTS_KEY`        | Google Cloud Text-to-Speech API key               |
| `AWS_ACCESS_KEY_ID`     | Amazon Polly access key ID                        |
| `AWS_SECRET_ACCESS_KEY` | Amazon Polly secret access key                    |
| `AWS_SESSION_TOKEN`     | Optional AWS temporary-credential token           |
| `AWS_REGION`            | Amazon Polly region                               |
| `AWS_DEFAULT_REGION`    | Polly region fallback when `AWS_REGION` is absent |

See [Models](/models/) and [TTS providers](/tts-providers/) for provider-specific resolution rules.

## Workspace configuration

A workspace contains `prompts/` and can include `anki-llm.yaml`:

```yaml title="anki-llm.yaml"
default_model: gemini-2.5-flash
reasoning_effort: low
```

The effective workspace is the workspace containing the current directory. When none contains it, `default_workspace` supplies a fallback. Its `prompts/`, `note-types/`, and `anki-llm.yaml` become available to workspace-aware commands.

Model precedence is CLI, workspace `default_model`, global `model`, then key-based detection. Reasoning effort precedence is CLI, environment, workspace, global, then unset. API base URL does not have a workspace key.

Read [Use workspaces](/workspaces/) for initialization, discovery, prompt selection, and version-control patterns.

## Common setup

```bash
anki-llm config set model gpt-5-mini
anki-llm config set reasoning_effort low
anki-llm config set default_workspace ~/anki
anki-llm config set anki_connect_url http://192.168.1.100:8765
anki-llm config set gemini_thinking_enabled false
```

Inspect the resolved environment with:

```bash
anki-llm doctor
```

## State, logs, snapshots, and cache

| Path | Contents |
| --- | --- |
| `~/.local/state/anki-llm/state.json` | Ephemeral application state, including the last prompt |
| `~/.local/state/anki-llm/logs/` | Automatic LLM session logs |
| `~/.local/state/anki-llm/note-type-snapshots/` | Backups created before note type pushes |
| `~/.cache/anki-llm/tts/` | Content-addressed synthesized audio cache |
| `<workspace>/note-types/<slug>/.sync-state.json` | Last-synced note type hash, locally gitignored |

These paths assume the standard home directory layout used by the current implementation.
