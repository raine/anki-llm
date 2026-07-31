---
title: "Verify translations"
description: "Review and replace translations across a large deck with a resumable file workflow."
---

This recipe repairs English translations in a 1,000-note Japanese deck. It uses
a local YAML file so you can inspect the model's work before changing Anki and
resume safely if processing is interrupted.

Assume the deck is named `Japanese Core 1k` and each note has `Japanese` and
`Translation` fields.

## Export the deck

Open Anki, then export the notes:

```sh
anki-llm export "Japanese Core 1k" --output notes.yaml
```

A row looks like this:

```yaml
- noteId: 1512345678901
  Japanese: 猫は机の上にいます。
  Translation: The cat is on the desk.
- noteId: 1512345678902
  Japanese: 彼は毎日公園を散歩します。
  Translation: He strolls in the park every day.
```

YAML keeps multiline fields readable and is easy to compare after processing.
Anki can be closed after export. See [Process a file](/process-file/) for the
file lifecycle and resume rules.

## Write the translation prompt

Create `prompt-ja-en.md`:

```markdown
---
output:
  field: Translation
  require_result_tag: true
---

You are an expert Japanese-to-English translator.

Translate this Japanese sentence into English: {Japanese}

Guidelines:
- Preserve nuance and meaning.
- Write natural, idiomatic English.
- When practical, preserve clues to the original Japanese grammar.

Briefly analyze the sentence and translation choices. Put only the final
translation inside <result></result> tags.
```

The model can reason in its response, but only the final tagged value reaches
the `Translation` field. See [Write prompts](/prompts/#isolate-a-result-from-explanatory-output)
for result-tag behavior.

:::tip
If you want the model to consider the existing translation, include it as a
separate labeled input such as `Existing translation: {Translation}` and ask it
to retain correct wording where appropriate.
:::

## Test a small sample

First verify placeholder expansion without calling the model:

```sh
anki-llm process-file notes.yaml \
  --output notes-translated.yaml \
  --prompt prompt-ja-en.md \
  --dry-run
```

Then process three translations and review the proposed changes:

```sh
anki-llm process-file notes.yaml \
  --output notes-translated.yaml \
  --prompt prompt-ja-en.md \
  --model gemini-2.5-flash \
  --preview
```

Check proper names, omitted subjects, tense, register, and sentences whose
meaning depends on context. Revise the prompt and repeat until the sample is
reliable.

## Process all notes

Run the full batch:

```sh
anki-llm process-file notes.yaml \
  --output notes-translated.yaml \
  --prompt prompt-ja-en.md \
  --model gemini-2.5-flash \
  --batch-size 10
```

The output is saved incrementally. If the command stops, rerun the same command
and output path. Completed rows are skipped, while failed rows remain eligible
for retry.

The terminal summary reports successes, failures, elapsed time, token totals,
and estimated cost. See the [`process-file` reference](/command-reference/#command-process-file)
for concurrency, limits, retries, and diagnostic logging.

## Review the result

Compare the original and processed files:

```sh
git diff --no-index notes.yaml notes-translated.yaml
```

Spot-check a representative sample, including short fragments, long sentences,
slang, names, and cards with HTML. If a systematic error appears, adjust the
prompt and run against a fresh output path or intentionally reprocess with the
force option.

:::caution
Fluent output can still be incorrect. Keep the original export until you have
reviewed the updated cards in Anki and are satisfied with the result.
:::

## Import the translations

Open Anki again and update the original notes:

```sh
anki-llm import notes-translated.yaml --deck "Japanese Core 1k"
```

The exported `noteId` values identify existing notes, so the import updates them
instead of creating replacements. The note type is inferred when the deck has a
single type. See the [`import` reference](/command-reference/#command-import) when you
need an explicit note type or key field.
