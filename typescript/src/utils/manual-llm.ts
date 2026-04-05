import clipboardy from 'clipboardy';
import chalk from 'chalk';
import readline from 'readline';

const MULTILINE_INPUT_TERMINATOR = 'END';

/**
 * Reads multiline input from stdin until a terminator string (`END`) is
 * entered on its own line.
 * This method avoids closing the stdin stream, allowing other libraries
 * like inquirer to use it afterwards.
 * @returns A promise that resolves with the stdin content.
 */
function readMultilineStdin(): Promise<string> {
  return new Promise((resolve) => {
    const lines: string[] = [];
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    });

    rl.on('line', (line) => {
      // A terminator string on its own line signals the end of input
      if (line.trim() === MULTILINE_INPUT_TERMINATOR) {
        rl.close();
      } else {
        lines.push(line);
      }
    });

    rl.on('close', () => {
      resolve(lines.join('\n').trim());
    });
  });
}

/**
 * Handles the manual workflow for getting an LLM response.
 * 1. Copies a prompt to the clipboard.
 * 2. Displays instructions for the user.
 * 3. Waits for the user to paste the response into stdin.
 * @param prompt The LLM prompt to be copied.
 * @returns A promise that resolves with the user-pasted LLM response.
 */
export async function getLlmResponseManually(prompt: string): Promise<string> {
  try {
    await clipboardy.write(prompt);
    console.log(chalk.green('✓ Prompt copied to clipboard.'));
  } catch {
    console.log(
      chalk.yellow(
        '\n⚠️  Could not copy to clipboard. Please copy the prompt manually below:',
      ),
    );
    console.log(chalk.gray('-'.repeat(60)));
    console.log(prompt);
    console.log(chalk.gray('-'.repeat(60)));
  }

  console.log(chalk.cyan('\n📋 Please follow these steps:'));
  console.log(
    chalk.cyan(
      '  1. Paste the prompt into your preferred LLM (ChatGPT, Claude, etc.).',
    ),
  );
  console.log(
    chalk.cyan('  2. Copy the full, raw JSON response from the LLM.'),
  );
  console.log(chalk.cyan('  3. Paste the response here in the terminal.'));
  console.log(
    chalk.cyan(
      `  4. Type "${MULTILINE_INPUT_TERMINATOR}" on a new line and press Enter to submit.`,
    ),
  );
  console.log(chalk.yellow('\nWaiting for LLM response...'));

  const response = await readMultilineStdin();

  if (!response) {
    throw new Error('No response received from stdin.');
  }

  console.log(chalk.green('✓ Response received. Processing...\n'));
  return response;
}
