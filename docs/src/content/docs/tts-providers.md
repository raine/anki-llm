---
title: "TTS providers"
description: "Configure credentials, voices, and settings for each TTS provider."
---

Select a provider with `--provider` in flag mode or `tts.provider` in a prompt. The accepted IDs are `openai`, `azure`, `google`, `amazon`, and `edge`. OpenAI is the default.

Credentials belong in environment variables or global configuration, not prompt YAML. In flag mode, credential flags take precedence over environment variables, which take precedence over `~/.config/anki-llm/config.json`.

## Settings at a glance

| Provider | Voice example | Region | `model` meaning | `speed` |
| --- | --- | --- | --- | --- |
| OpenAI | `alloy` | Unsupported | TTS model, default `gpt-4o-mini-tts` | Provider speed |
| Azure | `ja-JP-MasaruMultilingualNeural` | Required | Unsupported | Unsupported |
| Google | `ja-JP-Neural2-B` | Unsupported | Unsupported | `audioConfig.speakingRate` |
| Amazon | `Takumi` | Required | Polly engine | Unsupported |
| Edge | `ja-JP-NanamiNeural` | Unsupported | Unsupported | SSML prosody rate |

All providers support `format: mp3`. The current implementation writes MP3 audio. Furigana routing differs by provider: Azure uses SSML `<sub>`, while the other providers replace annotated kanji with plain kana.

## OpenAI

OpenAI resolves credentials in this order:

1. `--api-key`
2. `ANKI_LLM_API_KEY`
3. `OPENAI_API_KEY`

A custom `--api-base-url`, `ANKI_LLM_API_BASE_URL`, or global `api_base_url` can point OpenAI TTS requests at a compatible endpoint. A custom endpoint can run without a key if the server permits it.

Persist defaults:

```bash
anki-llm config set tts_provider openai
anki-llm config set tts_voice alloy
anki-llm config set tts_model gpt-4o-mini-tts
anki-llm config set tts_format mp3
```

```yaml
---
deck: English
note_type: Basic
field_map:
  front: Front
  back: Back
tts:
  target: Audio
  source:
    field: front
  provider: openai
  voice: alloy
  model: gpt-4o-mini-tts
  format: mp3
  speed: 1.0
---
```

Common voice IDs include `alloy`, `ash`, `ballad`, `coral`, `echo`, `fable`, `nova`, `onyx`, `sage`, and `shimmer`. Consult the [OpenAI TTS guide](https://platform.openai.com/docs/guides/text-to-speech) for the provider's current catalog.

## Azure Neural TTS

Azure needs a Speech subscription key and region.

| Source        | Key             | Region             |
| ------------- | --------------- | ------------------ |
| CLI           | `--api-key`     | `--azure-region`   |
| Environment   | `AZURE_TTS_KEY` | `AZURE_TTS_REGION` |
| Global config | `azure_tts_key` | `azure_tts_region` |

```bash
export AZURE_TTS_KEY="your-subscription-key"
export AZURE_TTS_REGION="eastus"

anki-llm tts Japanese \
  --field Audio \
  --text-field Reading \
  --provider azure \
  --voice ja-JP-MasaruMultilingualNeural \
  --azure-region eastus
```

```yaml
---
deck: Japanese::Vocab
note_type: VocabCard
field_map:
  reading: Reading
tts:
  target: Audio
  source:
    field: reading
  provider: azure
  voice: ja-JP-MasaruMultilingualNeural
  region: eastus
  format: mp3
---
```

Azure voices use `<locale>-<Voice>Neural` names. `region` is required. `model` and `speed` are rejected in prompt YAML and ignored as provider settings. Browse the [Azure language and voice list](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support).

## Google Cloud Text-to-Speech

Enable Text-to-Speech in a Google Cloud project and create an API key. Credentials resolve from `--api-key`, then `GOOGLE_TTS_KEY`, then the global `google_tts_key` setting.

```bash
export GOOGLE_TTS_KEY="your-google-cloud-key"
```

```yaml
---
deck: Japanese::Vocab
note_type: VocabCard
field_map:
  reading: Reading
tts:
  target: Audio
  source:
    field: reading
  provider: google
  voice: ja-JP-Neural2-B
  format: mp3
  speed: 1.0
---
```

Supply the complete voice name, such as `ja-JP-Neural2-B`, `en-US-Wavenet-D`, or `cmn-CN-Wavenet-A`. anki-llm derives `languageCode` from the first two segments. `region` and `model` are unsupported. `speed` is sent as `audioConfig.speakingRate`. See the [Google voice list](https://cloud.google.com/text-to-speech/docs/voices).

## Amazon Polly

Polly uses AWS credentials and a region.

| Source | Access key | Secret | Region | Session token |
| --- | --- | --- | --- | --- |
| CLI | `--aws-access-key-id` | `--aws-secret-access-key` | `--aws-region` | Environment only |
| Environment | `AWS_ACCESS_KEY_ID` | `AWS_SECRET_ACCESS_KEY` | `AWS_REGION`, then `AWS_DEFAULT_REGION` | `AWS_SESSION_TOKEN` |
| Global config | `aws_tts_access_key_id` | `aws_tts_secret_access_key` | `aws_tts_region` | Unsupported |

```bash
export AWS_ACCESS_KEY_ID="your-access-key-id"
export AWS_SECRET_ACCESS_KEY="your-secret-access-key"
export AWS_REGION="us-east-1"
```

```yaml
---
deck: Japanese::Vocab
note_type: VocabCard
field_map:
  reading: Reading
tts:
  target: Audio
  source:
    field: reading
  provider: amazon
  voice: Takumi
  region: us-east-1
  model: neural
  format: mp3
---
```

`voice` is a Polly `VoiceId`, such as `Joanna`, `Matthew`, `Takumi`, or `Mizuki`. `model` selects the Polly engine and accepts exactly `standard`, `neural`, `generative`, or `long-form`. `region` is required and `speed` is unsupported. A voice and region must support the selected engine. See the [Polly voice list](https://docs.aws.amazon.com/polly/latest/dg/voicelist.html).

## Microsoft Edge TTS

Edge uses Microsoft's consumer Read Aloud service. It needs no API key, region, or model.

```yaml
---
deck: Japanese::Vocab
note_type: VocabCard
field_map:
  reading: Reading
tts:
  target: Audio
  source:
    field: reading
  provider: edge
  voice: ja-JP-NanamiNeural
  format: mp3
  speed: 1.0
---
```

Voice IDs are Edge short names such as `en-US-JennyNeural` and `ja-JP-NanamiNeural`. `speed` becomes an SSML prosody rate. Prompt YAML rejects `region` and `model`.

Edge is an unofficial free provider backed by a consumer endpoint. Service availability and throttling can differ from paid APIs. If a large batch is throttled, retry with fewer concurrent requests, for example `--batch-size 1`.

## Browse exact voice IDs

Use [`anki-llm tts-voices`](/tts/#find-and-audition-voices) to filter the bundled catalog, audition a voice, and copy a valid provider-specific YAML scaffold.
