import chalk from 'chalk';
import type { ArgumentsCamelCase } from 'yargs';
import {
  readConfigFile,
  writeConfigFile,
  getConfigPath,
} from '../config-manager.js';
import type { Command } from './types.js';

const ACTION_CHOICES = ['get', 'set', 'list', 'path'] as const;
type ConfigAction = (typeof ACTION_CHOICES)[number];

type ConfigOptions = {
  action: ConfigAction;
  key?: string;
  value?: string;
};

type ConfigArgs = ArgumentsCamelCase<ConfigOptions>;

const command: Command<ConfigArgs> = {
  command: 'config <action> [key] [value]',
  describe: 'Manage persistent configuration settings',

  builder: (yargs) => {
    return yargs
      .positional('action', {
        describe: 'The configuration action',
        type: 'string',
        choices: ACTION_CHOICES,
        demandOption: true,
      })
      .positional('key', {
        describe: 'The configuration key',
        type: 'string',
      })
      .positional('value', {
        describe: 'The configuration value (for "set")',
        type: 'string',
      })
      .check((argv) => {
        if (argv.action === 'set' && (!argv.key || argv.value === undefined)) {
          throw new Error('The "set" action requires a key and a value.');
        }
        if (argv.action === 'get' && !argv.key) {
          throw new Error('The "get" action requires a key.');
        }
        return true;
      })
      .example('$0 config set model gpt-4o-mini', 'Set default model')
      .example('$0 config get model', 'Get the configured model')
      .example('$0 config list', 'List all configuration settings')
      .example('$0 config path', 'Show config file path');
  },

  handler: async (argv) => {
    const { action, key, value } = argv;

    try {
      switch (action) {
        case 'set': {
          if (!key || value === undefined) {
            throw new Error('The "set" action requires both key and value.');
          }
          const config = await readConfigFile();
          config[key] = value;
          await writeConfigFile(config);
          console.log(chalk.green(`✓ Set "${key}" to "${value}"`));
          console.log(chalk.dim(`  Config file: ${getConfigPath()}`));
          break;
        }

        case 'get': {
          if (!key) {
            throw new Error('The "get" action requires a key.');
          }
          const config = await readConfigFile();
          const configValue = config[key];
          if (configValue !== undefined) {
            console.log(configValue);
          } else {
            console.log(chalk.yellow(`Not set`));
          }
          break;
        }

        case 'list': {
          const config = await readConfigFile();
          if (Object.keys(config).length === 0) {
            console.log(chalk.yellow('No configuration settings found.'));
            console.log(chalk.dim(`Config file: ${getConfigPath()}`));
          } else {
            console.log(JSON.stringify(config, null, 2));
            console.log(chalk.dim(`\nConfig file: ${getConfigPath()}`));
          }
          break;
        }

        case 'path': {
          console.log(getConfigPath());
          break;
        }
      }
    } catch (error) {
      if (error instanceof Error) {
        console.log(chalk.red(`✗ Error: ${error.message}`));
      } else {
        console.log(chalk.red('✗ Unknown error:'), error);
      }
      process.exit(1);
    }
  },
};

export default command;
