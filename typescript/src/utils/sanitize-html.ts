import { marked } from 'marked';
import sanitizeHtml from 'sanitize-html';

// Configure marked once.
// We disable deprecated options and rely on our own sanitizer.
marked.setOptions({
  gfm: true, // Use GitHub Flavored Markdown.
  breaks: false, // Require newlines for paragraphs, standard behavior.
});

/**
 * Converts a string that may contain Markdown into HTML.
 * Existing HTML is passed through untouched.
 * Uses parseInline to avoid wrapping content in <p> tags.
 * @param content - The string to process.
 * @returns An HTML string.
 */
function convertMarkdownToHtml(content: string): string {
  // Use parseInline to process inline markdown without wrapping in <p> tags
  // The type assertion is safe as we are not using async extensions.
  return marked.parseInline(content) as string;
}

/**
 * Converts any markdown to HTML and then sanitizes the result to prevent XSS attacks.
 *
 * Strips dangerous tags and event handlers.
 * Allows common formatting tags used in Anki cards.
 *
 * @param dirtyContent - Raw content from LLM (may be HTML, Markdown, or mixed)
 * @returns Sanitized HTML safe for Anki
 */
export function sanitize(dirtyContent: string): string {
  // 1. Convert any Markdown to HTML.
  const htmlFromMarkdown = convertMarkdownToHtml(dirtyContent);

  // 2. Sanitize the resulting HTML.
  return sanitizeHtml(htmlFromMarkdown, {
    // Allow common formatting tags used in Anki cards
    allowedTags: [
      // Text formatting
      'b',
      'i',
      'u',
      'strong',
      'em',
      'mark',
      'small',
      'del',
      'ins',
      'sub',
      'sup',
      // Structure
      'p',
      'br',
      'div',
      'span',
      'hr',
      // Lists
      'ul',
      'ol',
      'li',
      // Tables
      'table',
      'thead',
      'tbody',
      'tr',
      'th',
      'td',
      // Links and images
      'a',
      'img',
      // Code
      'code',
      'pre',
      // Headers (sometimes used in notes)
      'h1',
      'h2',
      'h3',
      'h4',
      'h5',
      'h6',
    ],
    // Allow only safe attributes
    allowedAttributes: {
      a: ['href', 'title'],
      img: ['src', 'alt', 'title', 'width', 'height'],
      // Allow class and style for formatting (but sanitize them)
      '*': ['class', 'style'],
    },
    // Disallow all URL schemes except http, https, and data (for inline images)
    allowedSchemes: ['http', 'https', 'data'],
    // Disallow URL schemes in style attributes
    allowedStyles: {
      '*': {
        // Allow text colors and background colors
        color: [/^#[0-9a-f]{3,6}$/i, /^rgb\(/i, /^rgba\(/i],
        'background-color': [/^#[0-9a-f]{3,6}$/i, /^rgb\(/i, /^rgba\(/i],
        // Allow text alignment
        'text-align': [/^left$/i, /^right$/i, /^center$/i],
        // Allow font sizes
        'font-size': [/^\d+(?:px|em|%)$/],
      },
    },
  });
}

/**
 * Sanitizes all string values in a field object.
 * Useful for sanitizing all fields of a card before adding to Anki.
 *
 * @param fields - Object with field names as keys and HTML content as values
 * @returns New object with all values sanitized
 */
function arrayToListHtml(items: string[]): string {
  const listItems = items
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
    .map((item) => `<li>${item}</li>`)
    .join('');

  return listItems.length > 0 ? `<ul>${listItems}</ul>` : '';
}

export function sanitizeFields(
  fields: Record<string, string | string[]>,
): Record<string, string> {
  const sanitized: Record<string, string> = {};
  for (const [key, value] of Object.entries(fields)) {
    const content = Array.isArray(value) ? arrayToListHtml(value) : value;
    sanitized[key] = sanitize(content);
  }
  return sanitized;
}
