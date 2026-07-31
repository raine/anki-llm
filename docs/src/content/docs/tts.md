---
title: "Text-to-speech"
description: "Generate, preview, cache, and attach speech audio to Anki notes."
---

`anki-llm tts` synthesizes speech, uploads the audio to Anki's media store, and writes a `[sound:filename.mp3]` reference into a note field. It streams notes through AnkiConnect, so a staging file is unnecessary.

## Fill an audio field

Use flag mode for a one-off run. Exactly one deck or Anki query and exactly one text source are required.

```bash
anki-llm tts Japanese \
  --field Audio \
  --text-field Front \
  --voice alloy
```

This selects notes in `Japanese`, reads `Front`, and fills `Audio`. Add `--dry-run` and `--limit 10` when testing a voice or selection.

A template can combine fields:

```text title="speak.txt"
{Word}. {ExampleSentence}
```

```bash
anki-llm tts Japanese \
  --field Audio \
  --template speak.txt \
  --voice nova
```

Template placeholders are case-insensitive raw Anki field names. If a selection contains more than one note type, provide `--note-type` so field validation is unambiguous.

See the [command reference](/command-reference/) for every flag and [TTS providers](/tts-providers/) for credentials and provider-specific options.

## Keep TTS settings with a prompt

Prompt mode stores the deck's audio design beside its generation prompt. It is best for a maintained, version-controlled deck.

```yaml
---
deck: Japanese::Vocab
note_type: VocabCard
field_map:
  expression: Expression
  reading: Reading
  meaning: Meaning

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
prompt body...
```

Run the bulk fill with:

```bash
anki-llm tts --prompt prompts/japanese.yaml
```

The prompt supplies `deck`, `note_type`, voice, target, source, and provider. To
override only the selection, pass a positional deck after `tts` or use
`--query`:

```bash
anki-llm tts "Japanese::Review" --prompt prompts/japanese.yaml
anki-llm tts --query 'deck:Japanese tag:needs-audio' --prompt prompts/japanese.yaml
```

Prompt mode rejects CLI overrides for voice, model, format, target field,
source, provider, speed, and note type. Edit the prompt to change those values.

`tts.target` is an Anki field name. `tts.source.field` and placeholders in `tts.source.template` are `field_map` keys. A source must set exactly one of `field` or `template`.

## Generate cards with audio

The same `tts:` block is used by [`anki-llm generate`](/generate/). In the selection UI, press `p` to preview audio for the focused card when a system audio player is available. Audio for accepted cards is synthesized, uploaded, and placed in `tts.target` during import. The summary can replay imported audio.

TTS credentials are independent from LLM command flags. For example, `generate --api-base-url` can point to a local model while TTS uses `OPENAI_API_KEY` or Azure credentials.

## Control Japanese pronunciation with furigana

Annotate a CJK run with its intended reading:

```text
日本語[にほんご]
転がり込[こ]んだ
お父[とう]さん
```

Each bracketed reading binds to the immediately preceding CJK cluster, including clusters inside words. Azure receives SSML `<sub>` pronunciation aliases. OpenAI, Google, Amazon Polly, and Edge receive plain kana substitutions. Text without annotations passes through unchanged.

The annotations can come from manual editing, another tool, or generated card fields. A generation prompt can ask the model to emit `漢字[かんじ]` into the field selected by `tts.source`.

## Existing audio, normalization, and cache

By default, a note with a non-empty target field is skipped. This makes repeat runs fill gaps without replacing existing audio. Use `--force` to regenerate every matching note.

Before synthesis, anki-llm normalizes source values:

- HTML tags are stripped.
- Cloze markers such as `{{c1::answer}}` become `answer`.
- Existing `[sound:...]` tags are removed.
- HTML entities are decoded.
- Whitespace is collapsed.

Generated files are cached under `~/.cache/anki-llm/tts/`. An identical request reuses its cached audio and avoids another provider charge. Clear all cached TTS files with:

```bash
rm -rf ~/.cache/anki-llm/tts
```

## Find and audition voices

`anki-llm tts-voices` opens a browser over the bundled OpenAI, Azure, Google, Amazon Polly, and Edge catalogs.

```bash
anki-llm tts-voices
anki-llm tts-voices --lang ja -q "female neural"
anki-llm tts-voices --provider amazon -q neural
```

- Type to filter provider, voice ID, display name, language, gender, and tags. Every whitespace-separated token must match. OpenAI multilingual voices remain visible under language pre-filters.
- Use arrow keys or Page Up and Page Down to move.
- Press Space to synthesize and play a sample.
- Press Enter to copy a `tts:` YAML scaffold for the highlighted voice. The scaffold includes provider-specific region and Polly engine values when required. Fill in `target` and `source`, then keep browsing.
- Press Escape or Ctrl-C to exit.

Browsing needs no credentials. Previewing does. Providers without usable
credentials show as unavailable, and the detail pane identifies the missing
environment variable or config key. Preview audio uses the same cache as batch
TTS.
