---
deck: Default
note_type: Basic
field_map:
  front: Front
  kanji: Kanji
  reading: Reading
  explanation: Explanation
---

Generate {count} Anki flashcards for the Japanese word or phrase "{term}".

Return a JSON array of objects. Each object must have exactly these keys:
- "front": a short English definition or translation
- "kanji": the word written in kanji/kana
- "reading": the hiragana/katakana reading
- "explanation": one sentence explaining usage or nuance

Example for "{term}":
[
  {
    "front": "cat",
    "kanji": "猫",
    "reading": "ねこ",
    "explanation": "Common noun for a domestic cat; often used in casual speech."
  }
]

Now generate {count} cards for "{term}". Return only the JSON array, no other text.
