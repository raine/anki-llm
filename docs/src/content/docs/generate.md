---
title: "Generate cards"
description: "Create candidate cards, review them in the TUI, and import only the cards you choose."
---

`anki-llm generate` turns a word, phrase, or concept into several candidate
cards. Its terminal UI keeps generation and import separate so you can inspect,
edit, reject, or regenerate every candidate before it reaches Anki.

## Create a generation prompt

The recommended starting point is the interactive prompt wizard:

```sh
anki-llm workspace init
anki-llm generate-init
```

Choose a deck and note type. The wizard samples existing notes and asks an LLM
to create a prompt that matches their fields, style, and formatting. It saves
the result in the workspace `prompts/` directory unless you provide a specific
output path.

Review the generated file before using it. A generation prompt declares the Anki
destination and maps short model-facing keys to Anki fields:

```yaml
---
title: Japanese Vocabulary
description: Contextual sentence cards with readings
deck: Japanese::Vocabulary
note_type: Basic
field_map:
  front: Front
  kanji: Kanji
  reading: Reading
  explanation: Explanation
---
```

The body must use `{term}` and `{count}`, ask for a JSON array, and show an exact
example whose keys match `field_map`. Arrays returned as field values become
HTML lists during import.

For a runnable starting point, see the
[vocabulary generation example](https://github.com/raine/anki-llm/blob/main/examples/generate_vocab.md)
or the more detailed
[Japanese contextual example](https://github.com/raine/anki-llm/blob/main/examples/japanese_contextual.md).
The [prompt reference](/prompt-reference/) documents generation frontmatter.

:::tip
A capable model can produce a better initial template in `generate-init`. You
can use a different, cheaper model for everyday card generation.
:::

## Select a prompt

Launch from a workspace and omit `--prompt`:

```sh
anki-llm generate
```

If the workspace contains one prompt, it is selected automatically. If it
contains several, a picker shows each prompt's title and description. The last
selection is remembered and preselected in the next session. You can also pass
a prompt path explicitly.

See [Use workspaces](/workspaces/) for prompt discovery and default workspace
behavior.

## Generate one or many terms

You can provide a term on the command line:

```sh
anki-llm generate "会議"
```

Or omit it and enter terms in the TUI. For a batch, press <kbd>Tab</kbd> after
each term to queue it, then press <kbd>Enter</kbd> on the final term. Pasting
newline-separated terms creates the same queue.

Use <kbd>Ctrl+O</kbd> to open the filterable model picker. It includes known
pricing information and changes the model for subsequent requests in the same
session. Session token use and estimated cost remain visible in the sidebar.

![The generate TUI showing term entry, progress, and session cost](/generate-tui.webp)

## Review candidates

After generation, the selection screen lists candidates and shows the focused
card in full:

![The card selection screen with candidate checkboxes and a full field preview](/anki-llm-selection.webp)

The main review actions are:

- <kbd>Space</kbd> toggles the focused card.
- <kbd>a</kbd> and <kbd>n</kbd> select all or none.
- <kbd>e</kbd> opens the focused card in `$EDITOR`.
- <kbd>R</kbd> regenerates one card with your feedback.
- <kbd>r</kbd> generates more candidates for the same term.
- <kbd>t</kbd> generates candidates for another term.
- <kbd>d</kbd> removes a candidate.
- <kbd>c</kbd> copies a card to the clipboard.
- <kbd>z</kbd> toggles post-selection processing when the prompt configures it.
- <kbd>Enter</kbd> confirms the selected cards.

Press <kbd>?</kbd> for the complete shortcut list for the current screen.

### Handle duplicates deliberately

Before selection, `anki-llm` compares the generated value for the note type's
first field with existing notes in the configured deck. Exact matches receive a
`[dup]` marker and a field-by-field diff against the Anki note. Duplicates cannot
be selected normally. Press <kbd>f</kbd> only when you intentionally want to
force-select one.

:::caution
Duplicate detection uses an exact match on the first Anki field. It does not
identify paraphrases, spelling variants, or duplicates in another deck. Review
the complete card before importing it.
:::

## Add focused processing steps

Generation prompts may define `pre_select` and `post_select` LLM steps.
Pre-selection steps can normalize fields or reject poor candidates before you
review them. Post-selection steps can polish or check only the cards you chose.

See [Processing steps](/processing-steps/) for transforms, checks, verdicts,
ordering, cost, and limitations.

## Observe model thinking

When a supported Gemini, DeepSeek, or Grok model emits reasoning during the
primary generation request, the running screen displays it in a temporary
Thinking block. The block disappears when generation completes and its content
is excluded from raw prompt and response logs.

Gemini thinking can be disabled in configuration when you prefer the normal
non-thinking request path. See [Configuration](/configuration/) for the setting.

## Generate without an API key

Copy mode sends the filled prompt through your clipboard instead of an API:

```sh
anki-llm generate "今日" --prompt prompt.md --copy
```

1. Paste the copied prompt into a browser-based LLM.
2. Copy its complete JSON response.
3. Paste the response into the terminal.
4. Type `END` on its own line and press <kbd>Enter</kbd>.
5. Review the validated candidates as usual.

:::note
Copy mode does not support `pre_select` or `post_select` processing steps. The
pasted response still has to contain all keys declared by `field_map`.
:::

## Add TTS audio

A generation prompt can include a `tts:` block that names an audio target,
source, provider, and voice. When TTS is configured and a system audio player is
available:

- press <kbd>p</kbd> during selection to preview the focused card's audio;
- selected cards receive finalized audio at import time;
- press <kbd>p</kbd> on the summary to replay imported audio.

LLM and TTS credentials remain separate. For example, generation can use
OpenRouter while audio uses OpenAI or Azure credentials. See
[Text-to-speech](/tts/) and [TTS providers](/tts-providers/) for configuration.

## Choose a safe output path

The default interactive flow adds only confirmed cards to Anki. For additional
separation, export generated cards to YAML or CSV and import them later:

```sh
anki-llm generate "会議" --output cards.yaml
anki-llm import cards.yaml --deck "Japanese::Vocabulary"
```

A dry run prints generated cards without starting selection or import. Raw
request logging is useful for debugging but can contain card content and model
reasoning. See [`generate` in the command reference](/command-reference/#generate)
for all runtime and output options.
