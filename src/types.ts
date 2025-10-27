import type { Frontmatter } from './utils/parse-frontmatter.js';

export { Frontmatter }; // Re-export for convenience

export type CardCandidate = {
  fields: Record<string, string>;
  rawResponse: string;
};

export type ValidatedCard = CardCandidate & {
  isDuplicate: boolean;
  ankiFields: Record<string, string>; // Mapped to actual Anki field names
};

export type ImportResult = {
  successes: number;
  failures: number;
};
