---
title: "Frequently asked questions"
description: "Find answers about AnkiConnect, costs, and AnkiMCP."
---

## Why does anki-llm require AnkiConnect?

Anki has no built-in external API for reading and updating a collection. AnkiConnect supplies a local REST API that anki-llm uses to export notes, import changes, add generated cards, upload audio, and manage note type templates.

Anki must be running for collection operations. File processing can run while Anki is closed because it works against CSV or YAML. Read the [AnkiConnect guide](/ankiconnect/) for setup and direct queries.

## How much does processing cost?

Cost depends on model pricing, prompt length, note fields included in each request, generated output size, retries, and extra processing steps. There is no single per-deck price.

As an order-of-magnitude example, generating a substantial HTML grammar explanation for 1,000 Glossika EN-JP cards has cost roughly:

| Model                           | Approximate cost per 1,000 cards |
| ------------------------------- | -------------------------------: |
| `gemini-2.5-flash-lite`         |                            $0.35 |
| `deepseek-v4-flash`             |                            $0.45 |
| `gemini-3.1-flash-lite-preview` |                            $1.00 |
| `gemini-2.5-flash`              |                            $1.50 |
| `gpt-5-mini`                    |                            $3.00 |
| `grok-4.3`                      |                            $3.50 |

A short hint or translation can cost much less. A long multi-field response can cost more. For generation, three candidates with a moderately involved prompt have cost about $0.0002 per card with `gemini-2.5-flash-lite`, $0.001 with `gemini-2.5-flash`, and $0.0025 with `gpt-5-mini`.

These figures are examples, not quotes. Provider pricing and model behavior change. Gateway markups, cached tokens, reasoning tokens, free tiers, and provider discounts can change the final bill.

Test a representative sample before a large run:

```bash
anki-llm process-deck "My Deck" -p prompt.md --limit 20 --preview
```

anki-llm reports token usage and an estimate for models in its built-in pricing table. Extrapolate from the sample and check current provider pricing. Every `transform` or `check` step adds an LLM request for each card that reaches it. Unknown models still work but have no built-in estimate. See [Models and cost estimates](/models/#known-models-and-cost-estimates).

TTS billing is separate. Cached identical requests reuse local audio, but new voices, text, models, speeds, or provider settings can produce billable requests.

## How is anki-llm different from AnkiMCP?

AnkiMCP is an MCP server for conversational, interactive access from an MCP client. It is a natural fit when an assistant should inspect or create a small number of cards during a chat.

anki-llm is a CLI and terminal UI for bulk, repeatable workflows:

| Need | anki-llm | AnkiMCP |
| --- | --- | --- |
| Process thousands of notes | Batch concurrency, retries, resume, and atomic field updates | Usually agent-driven calls per task |
| Review changes as files | CSV and YAML export, processing, diff, and import | Conversation-oriented |
| Choose the model and endpoint per run | Any OpenAI-compatible API, including local servers | Usually follows the MCP client's model setup |
| Generate several candidates | Interactive selection and editing TUI | Conversational creation |
| Add speech audio | Integrated multi-provider TTS and caching | Depends on server tools |
| Edit card templates | Pull and push local HTML and CSS | Depends on server tools |
| Give a coding agent raw API access | `anki-llm query` returns JSON | Native MCP tools |

They can complement each other. Use AnkiMCP for conversational collection access and anki-llm for version-controlled prompts, scripted bulk transformations, reviewable files, generation sessions, TTS, and local note type editing.
