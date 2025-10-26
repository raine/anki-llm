import sanitizeHtml from 'sanitize-html';

/**
 * Sanitizes HTML content to prevent XSS attacks when syncing to AnkiWeb.
 *
 * Strips dangerous tags (<script>, <iframe>, <object>, <embed>) and event handlers.
 * Allows common formatting tags used in Anki cards.
 *
 * @param dirtyHtml - Raw HTML content from LLM
 * @returns Sanitized HTML safe for Anki
 */
export function sanitize(dirtyHtml: string): string {
  return sanitizeHtml(dirtyHtml, {
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
export function sanitizeFields(
  fields: Record<string, string>,
): Record<string, string> {
  const sanitized: Record<string, string> = {};
  for (const [key, value] of Object.entries(fields)) {
    sanitized[key] = sanitize(value);
  }
  return sanitized;
}
