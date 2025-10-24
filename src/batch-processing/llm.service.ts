import OpenAI from 'openai';
import chalk from 'chalk';
import type { Config } from '../config.js';
import type { RowData, TokenStats } from './types.js';
import { requireNoteId, fillTemplate, withTimeout } from './util.js';
import { log } from './logger.js';

/**
 * Extracts content from <result></result> XML tags in the response.
 * If requireResultTag is false, returns the raw response.
 * If requireResultTag is true, throws an error if tags are missing (triggering a retry).
 */
async function parseXmlResult(
  response: string,
  rowId: string,
  requireResultTag: boolean,
): Promise<string> {
  const match = response.match(/<result>([\s\S]*?)<\/result>/);
  if (match && match[1]) {
    const result = match[1].trim();
    await log(
      `Row ${rowId}: Successfully parsed result from XML tags (${result.length} chars)`,
      true,
    );
    return result;
  }

  // No XML tags found
  if (requireResultTag) {
    // Strict mode: throw error to trigger retry
    const errorMsg = `Row ${rowId}: Response missing required <result></result> tags. Full response: ${response}`;
    await log(errorMsg, true);
    console.log(
      chalk.yellow(
        `\n  ⚠️  Response missing <result></result> tags. Full response:\n${chalk.gray(response)}`,
      ),
    );
    throw new Error(
      `Response missing required <result></result> tags. Response preview: ${response.substring(0, 100)}...`,
    );
  } else {
    // Permissive mode: use raw response
    return response.trim();
  }
}

/**
 * Processes a single row using the LLM with retry logic
 */
export async function processSingleRow(params: {
  row: RowData;
  promptTemplate: string;
  config: Config;
  client: OpenAI;
  tokenStats: TokenStats;
}): Promise<string> {
  const { row, promptTemplate, config, client, tokenStats } = params;
  const rowId = requireNoteId(row);

  await log(`Row ${rowId}: Starting processing`, true);

  const prompt = fillTemplate(promptTemplate, row);
  await log(`Row ${rowId}: Generated prompt (${prompt.length} chars)`, true);

  await log(`Row ${rowId}: Sending request to ${config.model}`, true);

  // Track request timing
  const requestStartTime = Date.now();

  // Add 60 second timeout to API request to prevent infinite hangs
  const response = await withTimeout(
    client.chat.completions.create({
      model: config.model,
      messages: [
        {
          role: 'user',
          content: prompt,
        },
      ],
      temperature: config.temperature,
      ...(config.maxTokens && { max_tokens: config.maxTokens }),
    }),
    60000, // 60 second timeout
    `Request timeout after 60 seconds for row ${rowId}`,
  );

  const requestDurationMs = Date.now() - requestStartTime;
  const rawResult = response.choices[0]?.message?.content?.trim() || '';
  await log(
    `Row ${rowId}: Received response (${rawResult.length} chars) in ${requestDurationMs}ms (${(requestDurationMs / 1000).toFixed(2)}s)`,
    true,
  );

  // Track token usage
  if (response.usage) {
    tokenStats.input += response.usage.prompt_tokens;
    tokenStats.output += response.usage.completion_tokens;
    await log(
      `Row ${rowId}: Token usage - Input: ${response.usage.prompt_tokens}, Output: ${response.usage.completion_tokens}`,
      true,
    );
  }

  // Parse XML to extract result from <result></result> tags (or use raw response)
  const result = await parseXmlResult(
    rawResult,
    rowId,
    config.requireResultTag,
  );

  await log(`Row ${rowId}: Processing complete`, true);
  return result;
}
