import { writeFile } from 'fs/promises';
import inquirer from 'inquirer';
import chalk from 'chalk';
import { z } from 'zod';
import OpenAI from 'openai';
import type { Command } from './types.js';
import { ankiRequest, NoteInfo } from '../anki-connect.js';
import {
  getFieldNamesForModel,
  findModelNamesForDeck,
} from '../anki-schema.js';
import { slugifyDeckName } from '../batch-processing/util.js';
import { getProviderConfig, getApiKeyForModel } from '../config.js';

interface GenerateInitArgs {
  output?: string;
  model?: string;
}

/**
 * Suggests a short key name for an Anki field.
 * Examples: "English" -> "en", "Japanese" -> "jp"
 */
function suggestKeyForField(fieldName: string): string {
  const lower = fieldName.toLowerCase();

  // Common mappings
  const commonMappings: Record<string, string> = {
    english: 'en',
    japanese: 'jp',
    furigana: 'furigana',
    romaji: 'rom',
    context: 'context',
    notes: 'note',
    translation: 'translation',
    front: 'front',
    back: 'back',
    example: 'example',
    meaning: 'meaning',
  };

  if (commonMappings[lower]) {
    return commonMappings[lower];
  }

  // Default: take first 2-3 letters
  if (fieldName.length <= 3) {
    return lower;
  } else if (fieldName.length <= 6) {
    return lower.substring(0, 3);
  } else {
    return lower.substring(0, 4);
  }
}

/**
 * Generates a boilerplate prompt body with instructions and a one-shot example.
 * If sampleCard is provided, uses it as a real example from the deck.
 * Otherwise, generates generic placeholder values.
 */
function generatePromptBody(
  fieldKeys: string[],
  sampleCard?: Record<string, string>,
): string {
  // Use real sample card or create placeholder
  const exampleJson: Record<string, string> = sampleCard || {};
  if (!sampleCard) {
    for (const key of fieldKeys) {
      exampleJson[key] = `Example value for ${key}`;
    }
  }

  // Determine intro based on whether we have a real sample
  const intro = sampleCard
    ? 'You are an expert assistant who creates excellent Anki flashcards in the style shown below.'
    : 'You are an expert assistant who creates one excellent Anki flashcard for a vocabulary term.';

  return `${intro}
The term to create a card for is: **{term}**

IMPORTANT: Your output must be a single, valid JSON object and nothing else.
Do not include any explanation, markdown formatting, or additional text.
All field values must be strings.
For fields that require formatting (like lists or emphasis), generate a single string containing well-formed HTML.

Follow the structure and HTML formatting shown in this example precisely:

\`\`\`json
${JSON.stringify(exampleJson, null, 2)}
\`\`\`

Return only valid JSON matching this structure.

Tips for creating high-quality cards:
- Provide clear, concise definitions or translations
- Include natural, contextual examples
- Use HTML tags like <b>, <i>, <ul>, <li> for formatting when helpful
- For language learning: include pronunciation guides if relevant
- Keep the content focused and easy to review`;
}

/**
 * Generates a contextual prompt body using an LLM by analyzing sample cards.
 */
async function generateContextualPromptBody(
  deckName: string,
  sampleCards: Array<Record<string, string>>,
  fieldKeys: string[],
  userModel?: string,
): Promise<string> {
  // Determine model
  let model: string;

  if (userModel) {
    model = userModel;
  } else {
    // Auto-detect based on available API key
    const useGemini = !process.env.OPENAI_API_KEY && process.env.GEMINI_API_KEY;
    model = useGemini ? 'gemini-2.5-flash' : 'gpt-4o-mini';
  }

  // Get provider configuration and API key for the model
  const providerConfig = getProviderConfig(model);
  const apiKey = getApiKeyForModel(model);

  if (!apiKey) {
    throw new Error(
      `${providerConfig.recommendedApiKeyEnv} environment variable is required for model '${model}'`,
    );
  }

  const client = new OpenAI({
    apiKey,
    baseURL: providerConfig.baseURL,
  });

  // Build the meta-prompt
  const metaPrompt = `You are an expert prompt engineer creating a prompt template for another AI.
Your goal is to generate a comprehensive prompt body that instructs an AI to create new Anki cards matching the provided style.

**CONTEXT:**
The user wants to create new Anki cards for their deck named "${deckName}".
I have sampled some existing cards from their deck to show you the desired style, content, and structure.

**EXISTING CARD EXAMPLES:**
\`\`\`json
${JSON.stringify(sampleCards, null, 2)}
\`\`\`

**YOUR TASK:**

**Step 1: Deep Analysis**
Carefully analyze the examples to understand and deconstruct the underlying rules:

1. **Deck's Purpose & Style**: What is the subject matter? Is it formal, casual, technical? What is the pedagogical goal (e.g., teaching conversational Japanese, medical terminology)?

2. **Semantic Formatting**:
   - **Bold Tags (<b>)**: What is the rule for bolding? Is it only the focus term, or does it include surrounding words/particles? Are there multiple bolded elements?
   - **Lists (<ul>/<li>)**: How are lists used in explanations? Are they for definitions, examples, usage notes, or something else?
   - **Other HTML**: Any other HTML tags or patterns (headings, emphasis, etc.)?

3. **Linguistic & Technical Conventions (CRITICAL FOR LANGUAGE DECKS)**:
   - **Furigana Spacing (Japanese)**: If the deck uses furigana like \`漢字[かんじ]\`, analyze the spacing carefully:
     - Is there a space between kanji compounds? (e.g., \`体感[たいかん] 温度[おんど]\`)
     - Is there a space before particles? (e.g., \`温度[おんど] は\`)
     - Are there exceptions where there is **NO** space? (e.g., before auxiliary verbs like \`~ない\`, \`~ます\`, or certain particles like \`の\`, \`を\`)
     - This spacing is often critical for technical reasons (audio generation, ruby syntax). You MUST identify these patterns.
   - **Explanation Structure**: Analyze the content of explanation/note fields across all examples. What topics are consistently covered? Common themes include:
     - Formal vs informal usage
     - Common mistakes or pitfalls
     - Cultural context or nuances
     - Collocations or example sentences
     - Grammatical patterns
     - Variations of the word/phrase
   - Synthesize these recurring topics into a structured checklist.

**Step 2: Generate the Prompt Body**
Using your analysis, generate a high-quality prompt body with these sections:

1. **Persona & Goal**: Start with a concise instruction for the AI, mentioning the deck's likely purpose and any pedagogical goals you inferred (e.g., "Create a Japanese flashcard designed to teach conversational nuances with cultural context.").

2. **Term Placeholder**: State that the term to be defined will be provided via the **{term}** placeholder. Phrase this naturally (e.g., "The term to create a card for is: **{term}**").

3. **One-Shot Example**: Provide a single, plausible, **NEW** example in a JSON code block. This example must perfectly follow all the formatting, spacing, and content rules you identified. The JSON keys must be exactly: ${fieldKeys.join(', ')}.

4. **Boilerplate**: Include: "IMPORTANT: Your output must be a single, valid JSON object and nothing else. Do not include any explanation, markdown formatting, or additional text. All field values must be strings."

5. **Formatting Requirements**: Create a section that explicitly lists the formatting rules you discovered.
   - **This is critical.** Codify all patterns you observed.
   - For complex rules like furigana spacing, provide **CORRECT** and **INCORRECT** examples to eliminate ambiguity.
   - Example: "CORRECT: \`10度[ど]しかない\`, INCORRECT: \`10度[ど] しか ない\`"
   - Be specific about which fields use which formatting.

6. **Content Requirements**: Create a checklist of topics that explanation fields must cover, based on the recurring themes you found. Make this specific and actionable.
   - Example: "The explanation field must be a list that includes: 1. Formal vs informal usage, 2. A common collocation or example, 3. Any potential pitfalls."

**OUTPUT FORMAT:**
Return ONLY the raw text for the prompt body. Do NOT include frontmatter, markdown formatting for the entire block, or any explanations about your own process.`;

  try {
    const response = await client.chat.completions.create({
      model,
      messages: [
        {
          role: 'user',
          content: metaPrompt,
        },
      ],
      temperature: 0.7,
    });

    const generatedPrompt = response.choices[0]?.message?.content?.trim();
    if (!generatedPrompt) {
      throw new Error('Empty response from LLM');
    }

    return generatedPrompt;
  } catch (error) {
    // Enhanced error logging for debugging
    let errorMessage = 'LLM API call failed';
    if (error instanceof Error) {
      errorMessage = error.message;
      // Log additional details if available
      if ('status' in error) {
        console.error('   Status code:', error.status);
      }
      if ('error' in error) {
        console.error('   Error details:', JSON.stringify(error.error));
      }
    }
    throw new Error(`LLM API call failed: ${errorMessage}`);
  }
}

const command: Command<GenerateInitArgs> = {
  command: 'generate-init [output]',
  describe: 'Create a prompt template file by querying your Anki collection',

  builder: (yargs) => {
    return yargs
      .positional('output', {
        describe:
          'Path to save the prompt file (defaults to deck-name-prompt.md)',
        type: 'string',
      })
      .option('model', {
        alias: 'm',
        describe:
          'LLM model to use for prompt generation (e.g., gpt-4o, gemini-2.5-flash)',
        type: 'string',
      })
      .example('$0 generate-init', 'Create prompt file named after the deck')
      .example('$0 generate-init my-prompt.md', 'Save to custom location')
      .example(
        '$0 generate-init --model gemini-2.5-flash',
        'Use Gemini for prompt generation',
      );
  },

  handler: async (argv) => {
    try {
      console.log(chalk.cyan('\n✨ Welcome to anki-llm generate-init!\n'));
      console.log(
        chalk.gray(
          'This wizard will help you create a prompt template for generating Anki cards.\n',
        ),
      );

      // Check if we're in a TTY environment
      if (!process.stdout.isTTY) {
        console.error(
          chalk.red('❌ This command requires an interactive terminal (TTY).'),
        );
        process.exit(1);
      }

      // Step 1: Fetch and select deck
      console.log(chalk.cyan('📚 Fetching your Anki decks...\n'));
      const deckNames = await ankiRequest('deckNames', z.array(z.string()), {});

      if (deckNames.length === 0) {
        console.error(
          chalk.red('❌ No decks found in Anki. Create a deck first.'),
        );
        process.exit(1);
      }

      const { selectedDeck } = await inquirer.prompt<{
        selectedDeck: string;
      }>([
        {
          type: 'list',
          name: 'selectedDeck',
          message: 'Select the target deck:',
          choices: deckNames,
          pageSize: 15,
        },
      ]);

      console.log(chalk.green(`\n✓ Selected deck: ${selectedDeck}\n`));

      // Step 2: Fetch note types used in the selected deck
      console.log(chalk.cyan('📋 Fetching note types used in this deck...\n'));
      let modelNameChoices = await findModelNamesForDeck(selectedDeck);

      if (modelNameChoices.length === 0) {
        console.log(
          chalk.yellow(
            `⚠️  Deck "${selectedDeck}" has no notes. Showing all available note types instead.\n`,
          ),
        );

        // Fallback to all note types if deck is empty
        modelNameChoices = await ankiRequest(
          'modelNames',
          z.array(z.string()),
          {},
        );

        if (modelNameChoices.length === 0) {
          console.error(
            chalk.red('❌ No note types found in your Anki collection.'),
          );
          process.exit(1);
        }
      }

      let selectedNoteType: string;

      if (modelNameChoices.length === 1) {
        selectedNoteType = modelNameChoices[0];
        console.log(
          chalk.green(
            `✓ Auto-selected the only available note type: ${selectedNoteType}\n`,
          ),
        );
      } else {
        const answer = await inquirer.prompt<{
          selectedNoteType: string;
        }>([
          {
            type: 'list',
            name: 'selectedNoteType',
            message: 'Select the note type:',
            choices: modelNameChoices,
            pageSize: 15,
          },
        ]);
        selectedNoteType = answer.selectedNoteType;
        console.log(
          chalk.green(`\n✓ Selected note type: ${selectedNoteType}\n`),
        );
      }

      // Step 3: Fetch field names and create mapping
      console.log(chalk.cyan('🔍 Fetching fields...\n'));
      const fieldNames = await getFieldNamesForModel(selectedNoteType);

      console.log(
        chalk.gray(
          `Found ${fieldNames.length} field(s): ${fieldNames.join(', ')}\n`,
        ),
      );

      // Step 4: Create field mapping with auto-suggestion and review
      console.log(
        chalk.cyan(
          '🗺️  Creating field mapping (LLM JSON keys → Anki fields)...\n',
        ),
      );

      // Auto-suggest keys for all fields
      const suggestedKeys = fieldNames.map(suggestKeyForField);

      // Detect and resolve duplicate keys
      const keyCounts: Record<string, number> = {};
      const resolvedKeys = suggestedKeys.map((key) => {
        const count = keyCounts[key] || 0;
        keyCounts[key] = count + 1;
        if (count > 0) {
          return `${key}${count + 1}`; // e.g., exp, exp2, exp3
        }
        return key;
      });

      // Build initial fieldMap
      let fieldMap: Record<string, string> = {};
      for (let i = 0; i < fieldNames.length; i++) {
        fieldMap[resolvedKeys[i]] = fieldNames[i];
      }

      // Display proposed mapping
      console.log(chalk.gray('Proposed mapping:'));
      for (const [key, value] of Object.entries(fieldMap)) {
        console.log(chalk.gray(`  ${key} → ${value}`));
      }

      // Ask user to accept or customize
      const { acceptMapping } = await inquirer.prompt<{
        acceptMapping: boolean;
      }>([
        {
          type: 'confirm',
          name: 'acceptMapping',
          message: 'Accept this mapping?',
          default: true,
        },
      ]);

      if (!acceptMapping) {
        console.log(
          chalk.gray(
            '\nCustomize the mapping. Press Enter to keep the suggested key.\n',
          ),
        );

        // Clear and rebuild fieldMap with user input
        const customFieldMap: Record<string, string> = {};

        for (const fieldName of fieldNames) {
          const currentKey = Object.keys(fieldMap).find(
            (k) => fieldMap[k] === fieldName,
          )!;

          const { jsonKey } = await inquirer.prompt<{ jsonKey: string }>([
            {
              type: 'input',
              name: 'jsonKey',
              message: `JSON key for "${fieldName}":`,
              default: currentKey,
              validate: (input: string) => {
                if (!input.trim()) {
                  return 'JSON key cannot be empty';
                }
                if (!/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(input)) {
                  return 'Invalid key. Use letters, numbers, and underscores only.';
                }
                return true;
              },
            },
          ]);
          customFieldMap[jsonKey.trim()] = fieldName;
        }

        // Replace fieldMap with custom mapping
        fieldMap = customFieldMap;
      }

      console.log(chalk.green('\n✓ Field mapping complete\n'));

      // Step 5: Generate the prompt body (with LLM if possible)
      let body: string;
      console.log(
        chalk.cyan(
          '\n🧠 Attempting to generate a smart prompt based on your existing cards...\n',
        ),
      );
      console.log(
        chalk.gray(
          '(This requires OPENAI_API_KEY or GEMINI_API_KEY environment variable)\n',
        ),
      );

      try {
        // 1. Find notes in the deck
        const noteIds = await ankiRequest('findNotes', z.array(z.number()), {
          query: `deck:"${selectedDeck}"`,
        });

        if (noteIds.length < 1) {
          throw new Error('No cards found in deck to analyze.');
        }

        // 2. Sample 3 random cards (or all if fewer than 3)
        const sampleCount = Math.min(3, noteIds.length);
        const shuffled = noteIds.sort(() => 0.5 - Math.random());
        const sampleIds = shuffled.slice(0, sampleCount);

        console.log(
          chalk.gray(`  Analyzing ${sampleCount} sample card(s)...\n`),
        );

        // 3. Fetch full note info
        const notesInfo = await ankiRequest('notesInfo', z.array(NoteInfo), {
          notes: sampleIds,
        });

        // 4. Format examples using the fieldMap keys
        // Filter out auto-generated fields (like sound files)
        const filteredFieldMap: Record<string, string> = {};
        const skippedFields: string[] = [];

        for (const [jsonKey, ankiField] of Object.entries(fieldMap)) {
          // Check sample to see if this field contains auto-generated content
          const sampleValue = notesInfo[0]?.fields[ankiField]?.value || '';
          const isAutoGenerated = /^\[sound:.*\]$/.test(sampleValue.trim());

          if (!isAutoGenerated) {
            filteredFieldMap[jsonKey] = ankiField;
          } else {
            skippedFields.push(ankiField);
          }
        }

        if (skippedFields.length > 0) {
          console.log(
            chalk.gray(
              `  Skipping auto-generated field(s): ${skippedFields.join(', ')}\n`,
            ),
          );
        }

        const sampleCards = notesInfo.map((note) => {
          const card: Record<string, string> = {};
          for (const [jsonKey, ankiField] of Object.entries(filteredFieldMap)) {
            card[jsonKey] = note.fields[ankiField]?.value || '';
          }
          return card;
        });

        // 5. Call LLM to generate contextual prompt
        body = await generateContextualPromptBody(
          selectedDeck,
          sampleCards,
          Object.keys(filteredFieldMap),
          argv.model,
        );

        console.log(chalk.green('✓ Smart prompt generated successfully!\n'));

        // Update fieldMap to only include non-auto-generated fields
        fieldMap = filteredFieldMap;
      } catch (error) {
        console.log(
          chalk.yellow(
            '\n⚠️  Could not generate smart prompt. Falling back to generic template.',
          ),
        );
        console.log(
          chalk.gray(
            `   Reason: ${error instanceof Error ? error.message : 'Unknown error'}\n`,
          ),
        );
        body = generatePromptBody(Object.keys(fieldMap));
      }

      // Step 6: Create frontmatter and full content
      console.log(chalk.cyan('📝 Creating prompt file...\n'));

      const frontmatter = `---
deck: ${selectedDeck}
noteType: ${selectedNoteType}
fieldMap:
${Object.entries(fieldMap)
  .map(([key, value]) => `  ${key}: ${value}`)
  .join('\n')}
---`;

      const fullContent = `${frontmatter}\n\n${body}\n`;

      // Step 6: Save the file
      const defaultFilename = `${slugifyDeckName(selectedDeck)}-prompt.md`;
      const outputPath = argv.output || defaultFilename;
      await writeFile(outputPath, fullContent, 'utf-8');

      console.log(chalk.green(`✓ Prompt template saved to ${outputPath}\n`));

      // Step 7: Show example command
      console.log(chalk.cyan('🎉 Setup complete!\n'));
      console.log(chalk.gray('Try it out:'));
      console.log(
        chalk.white(`  anki-llm generate "example term" -p ${outputPath}\n`),
      );
      console.log(
        chalk.gray(
          'Edit the prompt file to customize the instructions for the LLM.',
        ),
      );
    } catch (error) {
      if (error instanceof Error) {
        // Handle user cancellation gracefully
        if (error.message.includes('User force closed')) {
          console.log(chalk.yellow('\n\nWizard cancelled by user.'));
          process.exit(0);
        }
        console.error(chalk.red(`\n❌ Error: ${error.message}`));
      } else {
        console.error(chalk.red('\n❌ An unknown error occurred'));
      }

      console.log(chalk.gray('\nMake sure:'));
      console.log(
        chalk.gray('  1. Anki Desktop is running with AnkiConnect installed'),
      );
      console.log(chalk.gray('  2. You have at least one deck and note type'));

      process.exit(1);
    }
  },
};

export default command;
