---
title: "Use workspaces"
description: "Keep prompts, note types, and project-specific model settings together."
---

A workspace gives `anki-llm` a predictable home for the files that define an
Anki project. It is an ordinary directory recognized by either:

- an `anki-llm.yaml` file, or
- a `prompts/` directory

A typical workspace looks like this:

```text
my-anki-workspace/
├── anki-llm.yaml
├── prompts/
│   ├── japanese-vocabulary.md
│   └── verify-translations.md
└── note-types/
    └── Japanese_Vocabulary/
        ├── note-type.yaml
        ├── style.css
        ├── Recognition.front.html
        └── Recognition.back.html
```

## Initialize a workspace

Create one in the current directory:

```sh
anki-llm workspace init
```

Or pass a directory:

```sh
anki-llm workspace init ~/anki
```

Initialization creates `anki-llm.yaml` and `prompts/` when they do not exist. It
preserves existing files, so it is safe to run again.

You can also create `prompts/` yourself. The directory alone is enough for
workspace discovery.

## Check workspace discovery

Run:

```sh
anki-llm workspace info
```

The command first checks the current directory. If that directory is not a
workspace, it checks the configured `default_workspace`. The report shows the
resolved root, prompts directory, model and reasoning effort when configured,
and the note-types directory when it exists.

:::note[Discovery scope]
Workspace discovery checks the current directory itself. Run commands from the
workspace root, or configure it as the default, rather than relying on a search
through parent directories.
:::

The current-directory workspace always wins over the default workspace.

## Use a default workspace from any directory

If one workspace contains your usual prompts and note types, configure it as the
fallback:

```sh
anki-llm config set default_workspace ~/anki
```

This makes its `prompts/`, `note-types/`, and `anki-llm.yaml` available when the
current directory is not itself a workspace.

Inspect the setting:

```sh
anki-llm config get default_workspace
```

Remove it with:

```sh
anki-llm config unset default_workspace
```

The default path must still contain `anki-llm.yaml` or `prompts/`. If it does
not, `workspace info` reports that it points to a non-workspace directory.

## Organize the prompts directory

Put reusable prompt files under `prompts/`. Commands such as `generate` can
resolve them without a `--prompt` flag:

```sh
anki-llm generate "今日"
```

Prompt selection follows these rules:

- one available prompt is selected automatically
- multiple prompts open an interactive picker
- the last-used prompt is remembered and preselected on the next run
- an explicit `--prompt <path>` selects that file instead

`generate-init` writes its generated prompt to the active workspace's
`prompts/` directory when you omit an output path:

```sh
anki-llm generate-init
```

Use clear filenames based on the deck and task, such as
`japanese-vocabulary-generate.md` or `spanish-verify-translations.md`.

## Configure `anki-llm.yaml`

The optional workspace manifest supports project-specific LLM defaults:

```yaml
default_model: gemini-2.5-flash
reasoning_effort: low
```

- `default_model` selects the model for commands run in this workspace.
- `reasoning_effort` is sent as the provider-neutral top-level
  `reasoning_effort` value on compatible chat completion requests.

The provider and model determine accepted reasoning values. Common values
include `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`. An unset
value lets the provider choose its default.

Workspace model settings override the global model configuration. For reasoning
effort, the resolution order is:

1. `--reasoning-effort`
2. `ANKI_LLM_REASONING_EFFORT`
3. workspace `reasoning_effort`
4. global `reasoning_effort`
5. unset

Use a command flag for one-off experiments and the manifest for a project-wide
choice. See [Configuration](/configuration/) for provider URLs, API keys, and
global settings.

:::caution[Keep credentials outside the workspace]
Do not put API keys in `anki-llm.yaml` or prompt frontmatter. Use provider
environment variables, `ANKI_LLM_API_KEY`, or the supported global credential
configuration. This keeps a version-controlled workspace free of secrets.
:::

## Add prompt metadata

Generation prompt frontmatter can include `title` and `description` to make the
interactive picker easier to scan:

```yaml
---
title: Japanese Vocabulary
description: Contextual sentence cards with readings
deck: Japanese::Vocabulary
note_type: Japanese (recognition)
field_map:
  en: English
  jp: Japanese
---
```

`title` is the human-readable picker label. `description` should briefly explain
the prompt's intended deck, content, or style. The operational metadata remains
in the same frontmatter:

- `deck` names the destination deck
- `note_type` names the Anki note type
- `field_map` maps LLM output keys to Anki fields

Processing prompts use an `output` block instead of generation's deck and field
mapping. See [Prompt reference](/prompt-reference/) before reusing a prompt with
a different command.

## Keep workspace files in version control

A workspace is well suited to Git because its durable configuration is plain
text. Commit:

- `anki-llm.yaml`
- files under `prompts/`
- note type manifests, HTML templates, and CSS under `note-types/`
- small example inputs or expected outputs that do not contain private material

Review prompt and model-setting changes before a run. A Git commit identifies the
exact instructions and defaults used to produce an important batch.

Treat collection data separately. Exported notes, query results, raw LLM logs,
and error logs can contain personal or copyrighted study material. Ignore them
or store them securely unless you have deliberately decided they belong in the
repository. Never commit API keys.

Each pulled note type also contains `.sync-state.json`. `anki-llm` gitignores
that synchronization state inside the note-type directory. Commit the manifest,
templates, and CSS instead.

For complete reusable examples, see the repository's
[`examples/` directory](https://github.com/raine/anki-llm/tree/main/examples).
