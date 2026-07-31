---
title: "Processing steps"
description: "Split card generation into ordered transforms and quality checks."
---

Processing steps let a generation prompt delegate focused work to additional LLM
calls. The main prompt can create diverse cards, while separate steps normalize
formatting, rewrite related fields, or evaluate quality.

Define steps under `processing` in generation prompt frontmatter:

```yaml
processing:
  pre_select: []
  post_select: []
```

- `pre_select` runs after initial generation and before the candidate selection
  screen.
- `post_select` runs after you confirm a selection and before import.

:::note
Processing steps apply to `generate` prompts. Batch prompts for `process-file`
and `process-deck` declare their outputs directly. See
[Write batch prompts](/prompts/).
:::

## Transform one field

A `transform` step asks the model to rewrite card data. Use `target` for one
`field_map` key:

```yaml
processing:
  pre_select:
    - type: transform
      target: reading
      model: gemini-2.5-flash-lite
      prompt: |
        Add furigana in Kanji[reading] form and insert natural bunsetsu spaces.

        Japanese: {kanji}
        English: {front}
```

The system requests a structured response for `reading` and applies it to the
candidate. `target` refers to the left side of `field_map`, not the destination
Anki field name.

## Transform several fields atomically

Use `writes` when one step should produce a coordinated field set:

```yaml
processing:
  pre_select:
    - type: transform
      writes: [reading, context]
      prompt: |
        Japanese sentence: {kanji}
        English meaning: {front}

        Return a furigana reading and a brief note about the social context.
```

The model must return every named key in one structured JSON response. Results
from the step are applied together to the in-memory candidate.

A transform must declare either `target` or `writes`, and each destination must
be a key in `field_map`. Use `target` for one field and `writes` for multiple
fields.

## Check card quality

A `check` evaluates the whole card without writing fields:

```yaml
processing:
  post_select:
    - type: check
      prompt: |
        Evaluate whether this Japanese sentence is natural and whether the
        English translation preserves its meaning.

        Japanese: {kanji}
        English: {front}
```

Do not add `target` or `writes` to a check. `anki-llm` supplies the structured
response instructions and expects a `result` plus a brief `reason`. The verdict
has one of three values:

- `pass`: keep the card in the normal flow.
- `flag`: keep the card but attach the reason for review.
- `reject`: discard the card.

A pre-selection flag appears as information in the candidate selection UI. A
post-selection flag routes the card to a review screen before import. A reject
removes the card immediately. If the request or response fails after retries,
the affected card is discarded and the error is shown in the processing log.

:::tip
Write check prompts as evaluation criteria, not response-format instructions.
The application adds the `pass`, `flag`, and `reject` JSON schema automatically.
:::

## Control ordering

Steps within each phase execute in their listed order. Every later step sees the
field values produced by earlier transforms:

```yaml
processing:
  pre_select:
    - type: transform
      target: reading
      prompt: |
        Add complete furigana to: {kanji}
    - type: check
      prompt: |
        Check that every kanji in {reading} has a correct reading for {kanji}.
```

The first step finishes across the candidate set before the second starts. Within
a single step, cards are processed concurrently, so completion order does not
change pipeline order. Each card and step is a separate LLM request and adds to
session token use and cost.

## Override the model per step

Add `model` to a step when a focused task can use a different model:

```yaml
processing:
  post_select:
    - type: check
      model: gemini-2.5-flash
      prompt: |
        Check the factual accuracy of this explanation: {explanation}
```

Without an override, the step uses the generation model. A model override changes
the model name but keeps the main request client's API base URL and credentials.
This works well with an endpoint such as OpenRouter that serves several models.
Make sure the configured provider can access every named model.

## Choose the phase deliberately

Use `pre_select` when the result should affect candidate review:

- normalize furigana, whitespace, or HTML;
- enrich a field that helps you choose between candidates;
- reject clearly invalid candidates before they clutter the selection screen.

Use `post_select` when the work is worthwhile only for accepted cards:

- run a more expensive quality check;
- add final formatting or explanatory detail;
- verify content immediately before import.

The selection screen lets you toggle skipping post-selection processing with
<kbd>z</kbd>. Treat that as an explicit safety bypass, especially when checks are
part of your normal quality policy.

## Know the limitations

- Processing is unavailable in `generate --copy` mode because every step needs
  an API request.
- Placeholders can read card keys from `field_map`; unknown placeholders fail
  processing.
- Transform destinations must already exist in `field_map`. Steps cannot add an
  undeclared Anki field.
- A failed processing call discards that card instead of importing a partially
  processed result.
- Step concurrency is internal and is not configured separately in prompt YAML.
- Additional steps increase latency and cost for every card that reaches them.

See the
[Japanese contextual example](https://github.com/raine/anki-llm/blob/main/examples/japanese_contextual.md)
for an ordered transform and check, then use [Generate cards](/generate/) to run
and review the pipeline.
