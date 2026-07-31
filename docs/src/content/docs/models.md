---
title: "Models"
description: "Configure LLM providers, endpoints, reasoning effort, and cost estimates."
---

anki-llm sends OpenAI-compatible Chat Completions requests. Any model name is accepted, and a custom base URL supports hosted compatible APIs and local servers.

## Built-in provider detection

The model prefix selects a built-in endpoint and provider-specific key:

| Model prefix | Endpoint | API key |
| --- | --- | --- |
| `gemini-` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` |
| `deepseek-` | `https://api.deepseek.com` | `DEEPSEEK_API_KEY` |
| `grok-` | `https://api.x.ai/v1` | `XAI_API_KEY` |
| Any other prefix | OpenAI default | `OPENAI_API_KEY` |

```bash
export OPENAI_API_KEY="..."
export GEMINI_API_KEY="..."
export DEEPSEEK_API_KEY="..."
export XAI_API_KEY="..."
```

With no explicit model, resolution is:

1. `--model`
2. `default_model` in the effective workspace's `anki-llm.yaml`
3. Global `model`
4. A key-based default: `gemini-2.5-flash`, `deepseek-v4-flash`, `grok-4.3`, or `gpt-5`, checked in that order

A model that does not match Gemini, DeepSeek, or Grok uses OpenAI defaults unless you configure a custom endpoint.

## OpenRouter

```bash
export ANKI_LLM_API_KEY="your-openrouter-key"
anki-llm generate "今日" \
  --api-base-url https://openrouter.ai/api/v1 \
  --model anthropic/claude-sonnet-4
```

Persist the endpoint and model:

```bash
anki-llm config set api_base_url https://openrouter.ai/api/v1
anki-llm config set model anthropic/claude-sonnet-4
```

Model names are forwarded unchanged. Use OpenRouter's provider-qualified model ID.

## Local servers

Ollama, llama.cpp, vLLM, and similar servers work when they expose `/v1/chat/completions`:

```bash
anki-llm generate "今日" \
  --api-base-url http://localhost:11434/v1 \
  --model llama3
```

A custom endpoint does not require a key from anki-llm. The server can still require authentication, in which case pass `--api-key` or set `ANKI_LLM_API_KEY`.

For safety, provider-specific keys are not sent to a custom endpoint. If `OPENAI_API_KEY` is set and `--api-base-url` points elsewhere, anki-llm does not forward that key unless it is supplied through the generic key setting.

## Other compatible APIs

```bash
anki-llm process-file input.yaml \
  --output output.yaml \
  --prompt prompt.md \
  --api-base-url https://api.together.xyz/v1 \
  --api-key your-key \
  --model meta-llama/Llama-3-70b-chat-hf
```

Together, Fireworks, Groq, proxies, and private gateways follow the same pattern. Compatibility depends on the endpoint accepting the request fields used by the selected command.

## Endpoint and key precedence

API base URL:

1. `--api-base-url`
2. `ANKI_LLM_API_BASE_URL`
3. Global `api_base_url`
4. Built-in provider detection

API key:

1. `--api-key`
2. `ANKI_LLM_API_KEY`
3. Provider-specific environment variable, only for an auto-detected endpoint

API keys are not stored in the global LLM configuration. Use environment variables or a command flag.

## Reasoning effort

Reasoning effort is an optional provider-neutral top-level field in the standard Chat Completions request.

```bash
anki-llm config set reasoning_effort low
export ANKI_LLM_REASONING_EFFORT=medium
```

```yaml title="anki-llm.yaml"
default_model: gpt-5.4-mini
reasoning_effort: high
```

```bash
anki-llm process-file input.csv \
  --output output.csv \
  --prompt prompt.md \
  --reasoning-effort xhigh
```

Resolution order is CLI, `ANKI_LLM_REASONING_EFFORT`, workspace, global config, then unset. Values are trimmed and must be non-empty. anki-llm does not enforce a fixed vocabulary because providers and models differ. Common values include `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`.

When unset, the request omits `reasoning_effort` and the provider chooses its default. A compatible server must accept the field if you set it.

## Known models and cost estimates

The built-in pricing table recognizes exact model IDs in these families:

- OpenAI: GPT-4.1, GPT-4o, GPT-5 through GPT-5.4, including listed mini and nano variants
- Google: Gemini 2.0 Flash, Gemini 2.5 Flash, Flash Lite and Pro, plus listed Gemini 3 preview models
- DeepSeek: `deepseek-v4-flash` and `deepseek-v4-pro`
- xAI: `grok-4.3`

Known models appear in the TUI picker when their provider key is available. A generic `ANKI_LLM_API_KEY` makes all known models available in the picker, and the configured model is included even when unknown.

Pricing is an exact-name lookup, not provider discovery and not a whitelist. Unknown or provider-qualified names still run, but their cost estimate is unavailable. Estimates multiply reported input and output token counts by the hard-coded per-million-token rates. DeepSeek uses cache-miss, undiscounted rates.

Treat every displayed cost as guidance:

- Provider prices can change after anki-llm ships.
- Cached input, batch discounts, taxes, free tiers, routing markups, and custom contracts are not modeled.
- OpenRouter and other gateways can price the same model differently.
- Providers can report usage differently, especially reasoning tokens.
- Extra processing steps make additional requests.

Check the provider's current pricing before a large run. Use `--limit` on a representative sample and extrapolate from the token and cost summary.
