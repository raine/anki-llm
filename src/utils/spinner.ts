import chalk from 'chalk';
import readline from 'node:readline';

const frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

export class Spinner {
  private frameIndex = 0;
  private intervalId?: NodeJS.Timeout;
  private text = '';
  private isActive = false;
  private readonly isEnabled = Boolean(process.stdout.isTTY);

  start(text: string): void {
    this.text = text;
    if (this.isActive) {
      this.stop();
    }

    this.isActive = true;

    if (!this.isEnabled) {
      console.log(chalk.cyan(`… ${text}`));
      return;
    }

    this.intervalId = setInterval(() => this.renderFrame(), 80);
    this.renderFrame(true);
  }

  update(text: string): void {
    this.text = text;
  }

  interrupt(callback: () => void): void {
    if (!this.isEnabled || !this.isActive) {
      callback();
      return;
    }

    this.clearLine();
    callback();
    this.renderFrame(true);
  }

  succeed(message?: string): void {
    this.stopWith(chalk.green('✓'), message);
  }

  fail(message?: string): void {
    this.stopWith(chalk.red('✗'), message);
  }

  stop(message?: string): void {
    this.stopWith(' ', message);
  }

  private renderFrame(force = false): void {
    if (!this.isEnabled || !this.isActive) {
      return;
    }

    if (!force) {
      this.frameIndex = (this.frameIndex + 1) % frames.length;
    }

    const frame = frames[this.frameIndex];
    let output = `${frame} ${this.text}`;
    const columns = process.stdout.columns;
    if (columns && columns > 1 && output.length >= columns) {
      output = output.slice(0, columns - 1);
    }

    this.clearLine();
    process.stdout.write(chalk.cyan(output));
  }

  private stopWith(symbol: string, message?: string): void {
    if (!this.isActive) {
      return;
    }

    this.isActive = false;
    if (this.intervalId) {
      clearInterval(this.intervalId);
      this.intervalId = undefined;
    }

    if (!this.isEnabled) {
      console.log(`${symbol} ${message ?? this.text}`);
      return;
    }

    this.clearLine();
    console.log(`${symbol} ${message ?? this.text}`);
  }

  private clearLine(): void {
    if (!this.isEnabled) {
      return;
    }
    readline.cursorTo(process.stdout, 0);
    readline.clearLine(process.stdout, 1);
  }
}
