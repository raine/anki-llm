import yaml from 'js-yaml';
import { z } from 'zod';

const QualityCheckSchema = z.object({
  field: z
    .string()
    .min(1, 'The target field for the quality check is required.'),
  prompt: z.string().min(1, 'A prompt for the quality check is required.'),
  model: z.string().optional(),
});

/**
 * Schema for the YAML frontmatter in prompt template files.
 * Defines the Anki deck, note type, and field mapping configuration.
 */
export const FrontmatterSchema = z.object({
  deck: z.string().min(1, 'Deck name is required'),
  noteType: z.string().min(1, 'Note type is required'),
  fieldMap: z
    .record(z.string(), z.string())
    .refine(
      (map) => Object.keys(map).length > 0,
      'fieldMap must have at least one key-value pair',
    ),
  qualityCheck: QualityCheckSchema.optional(),
});

export type Frontmatter = z.infer<typeof FrontmatterSchema>;

export interface ParsedPromptFile {
  frontmatter: Frontmatter;
  body: string;
}

/**
 * Parses a prompt template file containing YAML frontmatter and a prompt body.
 *
 * Expected format:
 * ```
 * ---
 * deck: My Deck
 * noteType: Basic
 * fieldMap:
 *   front: Front
 *   back: Back
 * ---
 *
 * Your prompt text here with {term} placeholder...
 * ```
 *
 * @param fileContent - The raw content of the prompt file
 * @returns Parsed frontmatter and body
 * @throws Error if frontmatter is missing, malformed, or invalid
 */
export function parseFrontmatter(fileContent: string): ParsedPromptFile {
  // Match frontmatter block: opening ---, content, closing ---
  const match = fileContent.match(/^---\s*\n([\s\S]+?)\n---\s*\n([\s\S]*)$/);

  if (!match) {
    throw new Error(
      'Invalid prompt file format. Expected YAML frontmatter enclosed by --- markers.\n\n' +
        'Example:\n' +
        '---\n' +
        'deck: My Deck\n' +
        'noteType: Basic\n' +
        'fieldMap:\n' +
        '  front: Front\n' +
        '  back: Back\n' +
        '---\n\n' +
        'Your prompt text here...',
    );
  }

  const frontmatterText = match[1];
  const body = match[2].trim();

  // Parse YAML
  let parsedYaml: unknown;
  try {
    parsedYaml = yaml.load(frontmatterText);
  } catch (error) {
    const message =
      error instanceof Error ? error.message : 'Unknown YAML parsing error';
    throw new Error(`Failed to parse YAML frontmatter: ${message}`);
  }

  // Validate with Zod
  try {
    const frontmatter = FrontmatterSchema.parse(parsedYaml);
    return { frontmatter, body };
  } catch (error) {
    if (error instanceof z.ZodError) {
      const issues = error.issues
        .map((i) => `  - ${i.path.join('.')}: ${i.message}`)
        .join('\n');
      throw new Error(
        `Invalid frontmatter structure:\n${issues}\n\n` +
          'Required fields: deck, noteType, fieldMap',
      );
    }
    throw error;
  }
}
