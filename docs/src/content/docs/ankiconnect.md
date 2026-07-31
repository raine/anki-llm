---
title: "AnkiConnect"
description: "Understand the local API that connects anki-llm to Anki."
---

[AnkiConnect](https://ankiweb.net/shared/info/2055492159) is an Anki add-on that exposes a local HTTP API. Anki itself does not provide an equivalent API for external tools. anki-llm uses AnkiConnect to read notes, update fields, add cards, manage media, and inspect note types.

Install the add-on, restart Anki, and leave Anki running while using commands that access the collection. The default endpoint is `http://127.0.0.1:8765`. A remote, WSL, or non-default setup can use:

```bash
anki-llm config set anki_connect_url http://192.168.1.100:8765
```

AnkiConnect has broad access to the open collection. Keep it bound to a trusted interface and review its origin and network configuration before allowing remote clients.

## Query the API

`anki-llm query` provides a JSON-oriented command-line interface to any AnkiConnect action:

```bash
anki-llm query deckNames
anki-llm query modelNames
anki-llm query findNotes '{"query":"deck:Japanese"}'
anki-llm query cardsInfo '{"cards":[1498938915662]}'
anki-llm query getDeckStats '{"decks":["Default"]}'
anki-llm query version
```

The first argument is the AnkiConnect action. The optional second argument is a JSON object containing that action's parameters. Output is JSON, which makes the command suitable for shell pipelines and coding agents.

Anki search strings use Anki's own query language. For example:

```bash
anki-llm query findNotes '{"query":"deck:Japanese tag:vocabulary -is:suspended"}'
```

The same search syntax is accepted by command options such as `process-deck --query` and `tts --query`. See [Anki's searching documentation](https://docs.ankiweb.net/searching.html) for operators, quoting rules, and date searches.

## Look up actions and parameters

Ask the installed AnkiConnect instance for its API reference:

```bash
anki-llm query docs
# `help` is an alias
anki-llm query help
```

This is useful when an agent needs to discover an action before calling it. The repository also vendors a snapshot of the reference in [ANKI_CONNECT.md](https://github.com/raine/anki-llm/blob/main/ANKI_CONNECT.md). The running add-on's `docs` response is the best match for the version installed in Anki.
