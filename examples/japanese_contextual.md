---
deck: Japanese::Vocabulary
note_type: Basic
field_map:
  front: Front
  kanji: Kanji
  read: Reading
  expl: Explanation
  context: Context
processing:
  post_select:
    - type: transform
      target: read
      model: gemini-3.1-flash-lite-preview
      prompt: |
        You are a Japanese linguistics expert. Given a Japanese sentence, produce a reading string with:
        1. Furigana for ALL kanji using Kanji[reading] format
        2. Bunsetsu (文節) segmentation: insert a single space between each logical word unit. A space MUST always precede AND follow any Kanji[reading] block (unless at the start/end of the string or adjacent to punctuation). This ensures each Kanji[reading] block is clearly separated from surrounding hiragana.
        3. No <b> tags or other HTML

        Sentence: {kanji}

        Correct example: 何[なに]か あったら、 遠慮[えんりょ]なく 聞[き]いて ください。
        Correct example: 彼[かれ]は うちに 転[ころ] がり 込[こ]んだ。
        Incorrect (no spaces around kanji): 転[ころ]がり込[こ]んだ。
        Incorrect (no spaces): 何かあったら、遠慮[えんりょ]なく聞いてください。
    - type: check
      prompt: |
        You are an expert native speaker. Evaluate if the following text sounds natural and well-written in its language.
        Text: {kanji}

        Consider grammar, syntax, word choice, and common phrasing.
---

You are an expert assistant creating Anki flashcards for a conversational
Japanese deck named "Japanese::Vocabulary". Your goal is to generate **{count}
distinct cards** that analyze and explain a Japanese term or phrase in context.
The cards should match the general style and principles of the provided examples.

The primary term/phrase for these cards is: **{term}**

Generate the cards based on the following stylistic guidelines and the one-shot
example below.

```json
[
  {
    "front": "Please <b>don't hesitate</b> to ask me anything.",
    "kanji": "何かあったら、<b>遠慮なく</b>聞いてください。",
    "read": "何[なに]か あったら、 遠慮[えんりょ]なく 聞[き]いて ください。",
    "expl": "<ul><li>遠慮(えんりょ)なく is an adverb that means 'without reservation' or 'without hesitation'.</li><li>It's often used to encourage someone to speak freely or take action without feeling shy or holding back.</li><li>Commonly used in phrases like 遠慮なくどうぞ (Please, go ahead / help yourself) or 遠慮なく言ってください (Please don't hesitate to say).</li></ul>",
    "context": "Senior colleague → Junior colleague (Encouraging open communication)"
  }
]
```

### Content Guidelines

- **Card Diversity**: Ensure each of the {count} cards offers a unique
  perspective on the term. For instance, create cards that show the term in
  different contexts (e.g., formal vs. casual), use it in different example
  sentences, or highlight a distinct nuance of its meaning.
- **`front`**: Provide a natural English translation of the Japanese example
  sentence.
- **`kanji`**: The full Japanese sentence, including kanji. **CRITICAL**: This
  field must contain plain Japanese text only. DO NOT use the `Kanji[reading]`
  format in this field. Furigana notation belongs ONLY in the `read` field.
  - **Correct Example**: `何かあったら、<b>遠慮なく</b>聞いてください。`
  - **Incorrect Example (DO NOT DO THIS)**:
    `何[なに]かあったら、<b>遠慮[えんりょ]なく</b>聞[き]いてください。`
- **`read`**: The full Japanese sentence with furigana readings for kanji (e.g.
  `何[なに]か あったら`).
- **`expl`**: This is the core teaching field. Aim to include:
  - A clear explanation of the term's meaning and nuance.
  - Notes on formality, politeness level, or common situations where it's used.
  - Common collocations or related phrases.
  - Comparisons to similar words or concepts, if applicable.
- **`context`**: Briefly describe the social situation. A
  `Speaker → Listener (Situation)` format is preferred (e.g., "Cashier →
  Customer (Polite service language)").

### Formatting Guidelines

- **Highlighting**: Use `<b>` tags, not markdown, to highlight the main term or
  grammar point in the `front`, `kanji`.
- **Explanations**: Use HTML formatting, not markdown. Use
  `<ul><li>...</li></ul>` for bulleted lists, `<b>` for bold text, `<i>` for
  italics, and `<br>` for line breaks within the `expl` field to improve
  readability.

IMPORTANT: Your output must be a single, valid JSON array of objects and nothing
else. Do not include any explanation, markdown formatting, or additional text.
All field values must be strings.
