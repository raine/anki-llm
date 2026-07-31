---
title: "Troubleshooting"
description: "Diagnose AnkiConnect, model, provider, rate-limit, and TTS failures."
---

## Start with doctor

```bash
anki-llm doctor
```

The report shows detected API keys, the resolved default model and reasoning effort, the effective workspace, AnkiConnect URL and connectivity, and TTS credentials. Secrets are masked.

For active provider checks, run:

```bash
anki-llm doctor --check
```

For each configured LLM provider, `--check` sends a one-token completion using a low-cost model. It checks authentication, model access, account balance, and rate-limit availability. The command exits non-zero if any check fails and makes a small billable request for each checked provider.

A warning beside the resolved model means its exact name is absent from the built-in pricing list. That can indicate a typo, but custom and newer model names are valid.

## AnkiConnect is unavailable

**Symptoms:** connection refused, a failed AnkiConnect doctor row, or commands that cannot list decks or notes.

1. Start Anki and keep it open.
2. Install and enable the AnkiConnect add-on.
3. Test the endpoint with `anki-llm query version`.
4. Confirm the configured URL with `anki-llm config get anki_connect_url`.
5. For WSL or a remote host, configure a reachable address:

   ```bash
   anki-llm config set anki_connect_url http://192.168.1.100:8765
   ```

6. Check firewall and AnkiConnect origin or bind settings. Expose the API only on a trusted network.

## An API key is missing or rejected

Run `anki-llm doctor` to see which variables are detected, then export the key in the same shell that runs anki-llm:

```bash
export OPENAI_API_KEY="..."
export GEMINI_API_KEY="..."
export DEEPSEEK_API_KEY="..."
export XAI_API_KEY="..."
```

For OpenRouter or another compatible service, use `ANKI_LLM_API_KEY` and set the base URL. Provider-specific keys are intentionally withheld from custom endpoints. See [Models](/models/#endpoint-and-key-precedence).

If the key is present but `doctor --check` fails, read the provider response for invalid credentials, missing model permission, exhausted balance, or account restrictions.

## The wrong model or endpoint is selected

Inspect `doctor`, then check every source that can override the value:

```bash
anki-llm config get model
anki-llm config get api_base_url
anki-llm workspace info
```

A workspace `default_model` takes precedence over global `model`. CLI flags take precedence over both. `ANKI_LLM_API_BASE_URL` takes precedence over global `api_base_url`.

Model names are accepted without a whitelist. If a provider reports "model not found," use the exact ID exposed by that provider or gateway. Provider-qualified OpenRouter names differ from direct-provider names.

A compatible endpoint must implement OpenAI `POST /v1/chat/completions`. If it rejects `reasoning_effort`, unset the option or choose a value supported by that server:

```bash
anki-llm config unset reasoning_effort
```

## Requests are rate-limited or time out

LLM and TTS operations retry transient 429 responses, server errors, and timeouts according to `--retries`. Reduce concurrency when failures persist:

```bash
anki-llm process-file input.yaml -o output.yaml -p prompt.md --batch-size 1
anki-llm tts Japanese --field Audio --text-field Front --voice alloy --batch-size 1
```

Re-run an interrupted `process-file` command with the same input and output. It skips complete rows unless `--force` is used. TTS also skips populated target fields, and its cache prevents identical successful requests from being billed again.

For paid APIs, confirm quota and balance with the provider. Edge TTS uses an unofficial consumer endpoint and can throttle large batches even without an account.

## A batch prompt fails rows

Common causes include:

- The prompt lacks YAML frontmatter.
- Both or neither of `output.field` and `output.fields` are declared.
- A `{placeholder}` does not match an input field.
- `require_result_tag: true` is set but the response lacks a complete final `<result>...</result>` pair.
- Multi-field output has missing, empty, non-string, or extra JSON keys.

Use a small preview and raw logging:

```bash
anki-llm process-file input.yaml \
  --output output.yaml \
  --prompt prompt.md \
  --preview \
  --log run.log
```

Multi-field updates are atomic. An invalid response leaves all declared fields unchanged and can be retried safely. See [Prompt reference](/prompt-reference/).

## TTS configuration fails

Check the active provider's required values:

- OpenAI: `OPENAI_API_KEY` or `ANKI_LLM_API_KEY`, unless a custom endpoint needs no authentication.
- Azure: `AZURE_TTS_KEY` and `AZURE_TTS_REGION`.
- Google: `GOOGLE_TTS_KEY` from a project with Text-to-Speech enabled.
- Amazon: access key, secret key, and AWS region. Temporary credentials also use `AWS_SESSION_TOKEN`.
- Edge: no credentials.

Prompt mode requires exactly one TTS source. Azure and Amazon require `region`. Azure, Google, and Edge reject `model`; Azure and Amazon reject `speed`. Amazon `model` must be a supported Polly engine.

Use the voice browser to validate names and credential availability:

```bash
anki-llm tts-voices --provider azure --lang ja
```

Browsing works without credentials. Pressing Space reports the missing setting or attempts a preview. A provider can also reject a valid voice when its locale, region, engine, or account access is incompatible.

## Audio text sounds wrong

Source fields are stripped of HTML, cloze markup, sound tags, entities, and extra whitespace before synthesis. For Japanese words with ambiguous kanji readings, add inline furigana such as `日本語[にほんご]`. See [Text-to-speech](/tts/#control-japanese-pronunciation-with-furigana).

Use `--force` only when replacing existing target audio is intended. If a fixed request keeps reusing unwanted cached audio, clear `~/.cache/anki-llm/tts/` and run it again.
