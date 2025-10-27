// Generic row data type - can hold any fields
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type RowData = Record<string, any>;

/**
 * Enhanced row with error tracking
 */
export type ProcessedRow = RowData & { _error?: string };

/**
 * Token usage statistics
 */
export type TokenStats = {
  input: number;
  output: number;
};
