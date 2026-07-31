---
title: "Generate vocabulary cards"
description: "Create, refine, and import contextual vocabulary cards for one or more terms."
---

This recipe creates contextual Japanese vocabulary cards for `会議` and adds only
the candidates you approve to `Japanese::Vocabulary`.

## Create a workspace prompt

From the directory where you keep deck configuration and prompts, run:

```sh
anki-llm workspace init
anki-llm generate-init
```

Choose `Japanese::Vocabulary` and the intended note type. The wizard samples
existing notes and generates a prompt with a matching `field_map`, output shape,
and style. Review the saved file in `prompts/` and refine its instructions or
one-shot JSON example.

For a prompt you can inspect immediately, see the
[vocabulary example](https://github.com/raine/anki-llm/blob/main/examples/generate_vocab.md).
The
[Japanese contextual example](https://github.com/raine/anki-llm/blob/main/examples/japanese_contextual.md)
shows a richer card and post-selection processing.

:::tip
The prompt's example is the strongest signal for field formatting. Make it a
realistic card with exactly the keys declared in `field_map`.
:::

## Launch the generator

Start the TUI from the workspace:

```sh
anki-llm generate
```

A single prompt is selected automatically. With multiple prompts, choose one in
the picker. Press <kbd>Ctrl+O</kbd> whenever you want to switch the model used by
later requests.

See [Generate cards](/generate/) for prompt discovery, model selection, thinking
display, and the complete review workflow.

## Enter one or more terms

Type `会議` and press <kbd>Enter</kbd>. To generate several terms in one session,
press <kbd>Tab</kbd> after each queued term and <kbd>Enter</kbd> on the last one.
You can also paste newline-separated terms.

The model returns several contextual candidates. Session token use and estimated
cost appear in the sidebar while requests run.

## Review every candidate

On the selection screen, move through the candidate list and inspect every
mapped field. Check that:

- the sentence uses `会議` naturally and reflects the intended meaning;
- the English translation matches the Japanese;
- readings and furigana use your deck's conventions;
- explanations add useful nuance instead of repeating the definition;
- HTML matches the card template.

Use <kbd>Space</kbd> to select a card, <kbd>e</kbd> to edit it in `$EDITOR`, and
<kbd>R</kbd> to regenerate it with feedback. Use <kbd>r</kbd> for more candidates
for `会議`, or <kbd>t</kbd> to generate another term.

Exact matches on the note type's first field are marked `[dup]` and shown as a
diff against the existing Anki note. Leave them unselected unless the new card
is intentional. Pressing <kbd>f</kbd> force-selects a duplicate.

:::caution
Duplicate detection is an exact first-field comparison. It cannot replace your
review for near-duplicates or semantically equivalent cards.
:::

## Use quality processing when needed

A generation prompt can run transforms and checks before or after selection.
For example, use a transform to normalize furigana and a post-selection check to
evaluate whether the Japanese sounds natural.

Post-selection work runs only for accepted cards, which keeps expensive checks
focused. A flag opens a review path, while a reject removes the card. See
[Processing steps](/processing-steps/) for the configuration and verdict rules.

## Preview audio and import

If the prompt contains a `tts:` block, press <kbd>p</kbd> to hear the focused
card before selection. Audio for selected cards is finalized when they are
imported.

Press <kbd>Enter</kbd> after selecting the cards you want. Confirm the final
review to add them to Anki. From the summary, press <kbd>p</kbd> to replay
imported audio, <kbd>n</kbd> for another term, or <kbd>q</kbd> to quit.

For an extra review boundary, generate to a local file instead of importing
directly:

```sh
anki-llm generate "会議" --output meeting-cards.yaml
```

Review the file, then import it:

```sh
anki-llm import meeting-cards.yaml --deck "Japanese::Vocabulary"
```

The [`generate` command reference](/command-reference/#command-generate) lists output,
count, retry, model, copy, logging, and dry-run options without duplicating them
here.
