import { describe, it, expect } from 'vitest';
import { sanitize, sanitizeFields } from './sanitize-html.js';

describe('sanitize', () => {
  it('converts markdown bold to HTML bold', () => {
    const input = 'This is **bold** text';
    const output = sanitize(input);
    expect(output).toBe('This is <strong>bold</strong> text');
  });

  it('converts markdown italic to HTML italic', () => {
    const input = 'This is *italic* text';
    const output = sanitize(input);
    expect(output).toBe('This is <em>italic</em> text');
  });

  it('does not convert markdown lists (inline parsing only)', () => {
    // parseInline doesn't process block-level markdown like lists
    // Users should provide HTML lists in the LLM prompt if needed
    const input = '- Item 1\n- Item 2';
    const output = sanitize(input);
    expect(output).not.toContain('<ul>');
    expect(output).toBe('- Item 1\n- Item 2');
  });

  it('does not wrap simple text in p tags', () => {
    const input = 'Simple text without markdown';
    const output = sanitize(input);
    expect(output).not.toContain('<p>');
    expect(output).toBe('Simple text without markdown');
  });

  it('does not wrap text with inline markdown in p tags', () => {
    const input = 'Text with **bold** and *italic*';
    const output = sanitize(input);
    expect(output).not.toContain('<p>');
    expect(output).toBe('Text with <strong>bold</strong> and <em>italic</em>');
  });

  it('preserves existing HTML tags', () => {
    const input = 'Text with <b>existing HTML</b>';
    const output = sanitize(input);
    expect(output).toContain('<b>existing HTML</b>');
  });

  it('handles mixed markdown and HTML', () => {
    const input = 'Text with **markdown bold** and <b>HTML bold</b>';
    const output = sanitize(input);
    expect(output).toContain('<strong>markdown bold</strong>');
    expect(output).toContain('<b>HTML bold</b>');
  });

  it('strips dangerous tags', () => {
    const input = '<script>alert("xss")</script>Safe text';
    const output = sanitize(input);
    expect(output).not.toContain('<script>');
    expect(output).toContain('Safe text');
  });

  it('strips event handlers', () => {
    const input = '<div onclick="alert(1)">Text</div>';
    const output = sanitize(input);
    expect(output).not.toContain('onclick');
  });
});

describe('sanitizeFields', () => {
  it('sanitizes all fields in an object', () => {
    const fields = {
      front: 'This is **bold**',
      back: 'This is *italic*',
    };
    const output = sanitizeFields(fields);
    expect(output.front).toBe('This is <strong>bold</strong>');
    expect(output.back).toBe('This is <em>italic</em>');
  });

  it('converts array fields into sanitized unordered lists', () => {
    const fields = {
      front: ['First fact', 'Second fact'],
    };
    const output = sanitizeFields(fields);
    expect(output.front).toBe(
      '<ul><li>First fact</li><li>Second fact</li></ul>',
    );
  });
});
