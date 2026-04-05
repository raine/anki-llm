import chalk from 'chalk';
import type { Config } from '../config.js';
import { MODEL_PRICING } from '../config.js';
import { calculateCost } from '../utils/llm-cost.js';
import type { ProcessedRow, TokenStats } from './types.js';

/**
 * Print summary statistics
 */
export function printSummary(
  processedRows: ProcessedRow[],
  tokenStats: TokenStats,
  config: Config,
  elapsedMs: number,
  options?: {
    errorLogPath?: string; // For direct mode
  },
) {
  const failures = processedRows.filter((r) => r._error);
  const successes = processedRows.length - failures.length;

  console.log('\n' + '='.repeat(60));
  console.log(chalk.bold('Summary'));
  console.log('='.repeat(60));
  console.log(chalk.green(`✓ Successful: ${successes}`));
  if (failures.length > 0) {
    console.log(chalk.red(`✗ Failed: ${failures.length}`));

    // Show error log path for direct mode
    if (options?.errorLogPath) {
      console.log(
        chalk.yellow(
          `\n⚠️  Failed notes logged to: ${chalk.bold(options.errorLogPath)}`,
        ),
      );
    } else {
      // Show error details for file mode
      console.log(chalk.yellow('\nFailed rows:'));
      failures.forEach((row) => {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
        const rowId =
          row.id || row.noteId || Object.values(row)[0] || 'unknown';
        console.log(chalk.yellow(`  - Row ${String(rowId)}: ${row._error}`));
      });
    }
  }

  console.log(`\n${chalk.bold('Token Usage:')}`);
  console.log(`  Input tokens:  ${tokenStats.input.toLocaleString()}`);
  console.log(`  Output tokens: ${tokenStats.output.toLocaleString()}`);
  console.log(
    `  Total tokens:  ${(tokenStats.input + tokenStats.output).toLocaleString()}`,
  );

  // Get model-specific pricing
  const pricing = MODEL_PRICING[config.model];
  const totalCost = calculateCost(
    config.model,
    tokenStats.input,
    tokenStats.output,
  );
  const inputCost =
    (tokenStats.input / 1_000_000) * pricing.inputCostPerMillion;
  const outputCost =
    (tokenStats.output / 1_000_000) * pricing.outputCostPerMillion;

  console.log(`\n${chalk.bold('Cost Breakdown:')}`);
  console.log(`  Model: ${config.model}`);
  console.log(
    `  Input cost:  $${inputCost.toFixed(4)} ($${pricing.inputCostPerMillion.toFixed(2)}/M tokens)`,
  );
  console.log(
    `  Output cost: $${outputCost.toFixed(4)} ($${pricing.outputCostPerMillion.toFixed(2)}/M tokens)`,
  );
  console.log(chalk.bold(`  Total cost:  $${totalCost.toFixed(4)}`));

  console.log(`\n${chalk.bold('Performance:')}`);
  console.log(`  Total time: ${(elapsedMs / 1000).toFixed(2)}s`);
  console.log(
    `  Avg time per row: ${(elapsedMs / processedRows.length).toFixed(0)}ms`,
  );
}
