import type { Frontmatter } from './utils/parse-frontmatter.js';

export { Frontmatter }; // Re-export for convenience

export interface CardCandidate {
  fields: Record<string, string>;
  rawResponse: string;
}

export interface ValidatedCard extends CardCandidate {
  isDuplicate: boolean;
  ankiFields: Record<string, string>; // Mapped to actual Anki field names
}

export interface ImportResult {
  successes: number;
  failures: number;
}
