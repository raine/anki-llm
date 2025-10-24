import type { CommandModule } from 'yargs';

/**
 * Type-safe command module definition for the CLI.
 * All commands should export a default object of this type.
 */
export type Command<T = object> = CommandModule<object, T>;
