import { describe, it, expect } from 'vitest';
import { stripHtmlPreserveBold } from './selector.js';

const BOLD_START_MARK = '\u0000';
const BOLD_END_MARK = '\u0001';

describe('stripHtmlPreserveBold', () => {
  it('preserves bold tags using markers', () => {
    const input = 'Text with <b>bold</b> content';
    const output = stripHtmlPreserveBold(input);
    expect(output).toContain(BOLD_START_MARK);
    expect(output).toContain(BOLD_END_MARK);
    expect(output).toBe(
      `Text with ${BOLD_START_MARK}bold${BOLD_END_MARK} content`,
    );
  });

  it('removes all other HTML tags', () => {
    const input = 'Text with <div>nested</div> <span>content</span>';
    const output = stripHtmlPreserveBold(input);
    expect(output).not.toContain('<div>');
    expect(output).not.toContain('<span>');
    expect(output).toBe('Text with nested content');
  });

  it('adds spaces between list items', () => {
    const input = '<ul><li>Item 1</li><li>Item 2</li><li>Item 3</li></ul>';
    const output = stripHtmlPreserveBold(input);
    expect(output).toBe('Item 1 Item 2 Item 3');
    expect(output).not.toContain('Item 1Item 2');
  });

  it('adds spaces after paragraph tags', () => {
    const input = '<p>First paragraph</p><p>Second paragraph</p>';
    const output = stripHtmlPreserveBold(input);
    expect(output).toBe('First paragraph Second paragraph');
  });

  it('adds spaces after div tags', () => {
    const input = '<div>First div</div><div>Second div</div>';
    const output = stripHtmlPreserveBold(input);
    expect(output).toBe('First div Second div');
  });

  it('handles br tags by adding spaces', () => {
    const input = 'Line 1<br>Line 2<br>Line 3';
    const output = stripHtmlPreserveBold(input);
    expect(output).toBe('Line 1 Line 2 Line 3');
  });

  it('cleans up multiple consecutive spaces', () => {
    const input = '<li>Item 1</li>  <li>Item 2</li>';
    const output = stripHtmlPreserveBold(input);
    expect(output).toBe('Item 1 Item 2');
    expect(output).not.toContain('  ');
  });

  it('trims whitespace from start and end', () => {
    const input = '  <p>Content</p>  ';
    const output = stripHtmlPreserveBold(input);
    expect(output).toBe('Content');
  });

  it('handles mixed bold and list items', () => {
    const input = '<ul><li><b>Bold item 1</b></li><li>Regular item 2</li></ul>';
    const output = stripHtmlPreserveBold(input);
    expect(output).toContain(BOLD_START_MARK);
    expect(output).toContain(BOLD_END_MARK);
    expect(output).toBe(
      `${BOLD_START_MARK}Bold item 1${BOLD_END_MARK} Regular item 2`,
    );
  });
});
