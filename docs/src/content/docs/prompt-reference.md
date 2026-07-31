---
title: "Prompt reference"
description: "Look up batch, generation, processing, and TTS prompt syntax."
---

anki-llm has two prompt schemas. `process-file` and `process-deck` use batch output frontmatter. `generate` uses deck, note type, field mapping, optional processing, and optional TTS frontmatter. Both require a YAML block delimited by `---` at the start of the file. Use only the keys documented below; strict frontmatter blocks reject unknown entries.

## Batch processing prompts

Used by [`process-file`](/process-file/) and [`process-deck`](/process-deck/).

### Schema

```yaml
---
title: Optional title
description: Optional description
output:
  field: Translation
  require_result_tag: false
---
Prompt body
```

| Key | Type | Required | Rules |
| --- | --- | --- | --- |
| `title` | string | No | Reserved metadata |
| `description` | string | No | Reserved metadata |
| `output.field` | string | One output form | One non-empty destination field |
| `output.fields` | string list | One output form | At least two non-empty, unique destination fields |
| `output.require_result_tag` | boolean | No | Defaults to `false`; valid only with `output.field` |

Declare exactly one of `output.field` or `output.fields`.

### Single-field output

```text
---
output:
  field: Translation
  require_result_tag: true
---
Translate: {Japanese}
Existing translation: {Translation}
Wrap the final answer in <result></result> tags.
```

The body can reference raw Anki or input-file field names with `{Field}`. Matching is case-insensitive. An unknown placeholder fails that row.

With `require_result_tag: false`, the plain response is written. With `true`, only content inside the last `<result>...</result>` pair is written. A response without a complete pair fails and follows normal retry behavior.

### Multi-field output

```text
---
output:
  fields:
    - Reading
    - Explanation
    - KanjiBreakdown
---
Sentence: {Kanji}
Translation: {Front}
Generate all requested fields.
```

One request must return a JSON object with exactly the declared keys:

```json
{
  "Reading": "日本語[にほんご]を勉強[べんきょう]する。",
  "Explanation": "A neutral statement about studying Japanese.",
  "KanjiBreakdown": "<ul><li>勉強: study</li></ul>"
}
```

Every declared value must be a non-empty string. Missing, empty, non-string, or undeclared fields invalidate the response. Validation happens before any update, so all declared fields change together or none do. Undeclared note fields are preserved. Source-only staging fields remain in `process-file` output, and `import` ignores fields absent from the destination note type.

## Generation prompts

Used by [`generate`](/generate/).

### Complete frontmatter shape

```yaml
---
title: Japanese Vocabulary
description: Contextual sentence cards with readings
deck: Japanese::Vocabulary
note_type: Japanese (recognition)
field_map:
  en: English
  jp: Japanese
  reading: Reading

processing:
  pre_select:
    - type: transform
      target: reading
      model: gpt-4o-mini
      prompt: |
        Add Kanji[reading] annotations to: {jp}
  post_select:
    - type: check
      prompt: |
        Check whether this sentence is natural: {jp}

tts:
  target: Audio
  source:
    field: reading
  provider: openai
  voice: alloy
  model: gpt-4o-mini-tts
  format: mp3
  speed: 1.0
---
Generate {count} cards for {term} as a JSON array.
```

### Core keys

| Key | Type | Required | Meaning |
| --- | --- | --- | --- |
| `title` | string | No | Label in the workspace prompt picker; filename is the fallback |
| `description` | string | No | Supporting text in the prompt picker |
| `deck` | string | Yes | Exact destination Anki deck name |
| `note_type` | string | Yes | Exact destination Anki note type name |
| `field_map` | map | Yes | Non-empty map from LLM JSON keys to Anki field names |
| `processing` | object | No | Pre-selection and post-selection LLM steps |
| `tts` | object | No | Shared generation and bulk-TTS audio settings |

The prompt body must use `{term}` and `{count}`. It should require one JSON array of card objects whose keys are the left side of `field_map`. String values are used directly. JSON arrays in field values become `<ul><li>` HTML lists during import.

Example response for `field_map: { en: English, jp: Japanese }`:

```json
[
  {
    "en": "How was your day?",
    "jp": "今日はどうでしたか？"
  }
]
```

## Processing steps

`processing.pre_select` runs after generation and before card selection. `processing.post_select` runs after selection and before final import. Steps in a phase run in order, and later steps see earlier updates. Cards within a step are processed concurrently. Processing is unavailable with `generate --copy`.

### Transform

A transform rewrites one or more `field_map` keys. Use exactly one write form:

```yaml
processing:
  pre_select:
    - type: transform
      target: reading
      model: gpt-4o-mini
      prompt: "Add furigana to {jp}"
```

```yaml
processing:
  pre_select:
    - type: transform
      writes: [reading, context]
      prompt: |
        Sentence: {jp}
        Produce a reading and a brief context note.
```

| Key      | Required       | Rules                                                    |
| -------- | -------------- | -------------------------------------------------------- |
| `type`   | Yes            | `transform`                                              |
| `target` | One write form | One `field_map` key                                      |
| `writes` | One write form | One or more `field_map` keys                             |
| `prompt` | Yes            | Non-empty; all card fields are available as placeholders |
| `model`  | No             | Model override for this step                             |

A multi-field transform receives structured output for the declared write keys.

### Check

```yaml
processing:
  post_select:
    - type: check
      model: gemini-2.5-flash-lite
      prompt: "Evaluate whether this is natural Japanese: {jp}"
```

A check must not declare `target` or `writes`. anki-llm adds the response contract and expects `result` plus `reason`. Results are:

- `pass`: continue normally.
- `flag`: keep the card with a warning. Pre-selection flags are informational; post-selection flags open a review screen.
- `reject`: discard the card.

## Shared `tts` block

The same block drives audio during `generate` import and bulk filling with `tts --prompt`.

| Key | Type | Required | Rules |
| --- | --- | --- | --- |
| `target` | string | Yes | Non-empty Anki field name, not a `field_map` key |
| `source` | object | Yes | Exactly one of `field` or `template` |
| `source.field` | string | One source form | Must be a `field_map` key |
| `source.template` | string | One source form | Placeholders must be `field_map` keys, case-insensitive |
| `voice` | string | Yes | Non-empty provider voice identifier |
| `provider` | string | No | `openai` by default; also `azure`, `google`, `amazon`, `edge` |
| `region` | string | Provider-specific | Required for Azure and Amazon, forbidden otherwise |
| `model` | string | Provider-specific | OpenAI model or Amazon engine; forbidden for Azure, Google, Edge |
| `format` | string | No | Defaults to `mp3` |
| `speed` | number | No | Must be greater than zero; forbidden for Azure and Amazon |

Amazon `model` accepts `standard`, `neural`, `generative`, or `long-form`. Provider credentials stay outside prompt files. See [TTS providers](/tts-providers/) for exact resolution and examples.

## Placeholder scopes

| Context                     | Placeholder names                               |
| --------------------------- | ----------------------------------------------- |
| Batch prompt body           | Raw Anki or input-file fields, case-insensitive |
| Generation body             | `{term}` and `{count}`                          |
| Processing prompt           | `field_map` keys for all card fields            |
| `tts.source.template`       | `field_map` keys, case-insensitive              |
| TTS flag-mode template file | Raw Anki fields, case-insensitive               |
