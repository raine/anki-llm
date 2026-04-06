use std::collections::HashMap;

/// Convert markdown to HTML and sanitize the result.
pub fn sanitize_html(content: &str) -> String {
    let html = markdown_to_html(content);
    clean_html(&html)
}

/// Convert markdown to HTML using inline-only parsing.
///
/// Filters out block-level Paragraph wrapping to match the behaviour of
/// `marked.parseInline()` in the TypeScript version: plain text stays as-is
/// instead of being wrapped in `<p>` tags.
fn markdown_to_html(content: &str) -> String {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let parser = pulldown_cmark::Parser::new(content).filter(|event| {
        !matches!(
            event,
            Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph)
        )
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    html.trim_end().to_string()
}

/// Sanitize HTML, allowing only safe tags and attributes.
fn clean_html(html: &str) -> String {
    use std::collections::{HashMap, HashSet};

    let mut tag_attributes: HashMap<&str, HashSet<&str>> = HashMap::new();
    tag_attributes.insert("a", ["href", "title"].iter().copied().collect());
    tag_attributes.insert(
        "img",
        ["src", "alt", "title", "width", "height"]
            .iter()
            .copied()
            .collect(),
    );

    ammonia::Builder::default()
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .link_rel(None)
        .url_schemes(["http", "https", "data"].iter().copied().collect())
        .generic_attributes(["class", "style"].iter().copied().collect())
        .tag_attributes(tag_attributes)
        .clean(html)
        .to_string()
}

const ALLOWED_TAGS: &[&str] = &[
    "b", "i", "u", "strong", "em", "mark", "small", "del", "ins", "sub", "sup", "p", "br", "div",
    "span", "hr", "ul", "ol", "li", "table", "thead", "tbody", "tr", "th", "td", "a", "img",
    "code", "pre", "h1", "h2", "h3", "h4", "h5", "h6",
];

/// Convert a string array to an HTML unordered list.
fn array_to_list_html(items: &[String]) -> String {
    let list_items: String = items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| format!("<li>{item}</li>"))
        .collect();

    if list_items.is_empty() {
        String::new()
    } else {
        format!("<ul>{list_items}</ul>")
    }
}

/// Sanitize all fields in a card. Array values become `<ul>` lists.
pub fn sanitize_fields(fields: &HashMap<String, serde_json::Value>) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for (key, value) in fields {
        let content = match value {
            serde_json::Value::Array(arr) => {
                let strings: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                array_to_list_html(&strings)
            }
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        result.insert(key.clone(), sanitize_html(&content));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_markdown_bold() {
        let result = sanitize_html("**hello**");
        assert!(result.contains("<strong>hello</strong>"));
    }

    #[test]
    fn sanitize_strips_script() {
        let result = sanitize_html("<script>alert('xss')</script>hello");
        assert!(!result.contains("script"));
        assert!(result.contains("hello"));
    }

    #[test]
    fn array_to_list() {
        let items = vec!["one".into(), "two".into()];
        let html = array_to_list_html(&items);
        assert_eq!(html, "<ul><li>one</li><li>two</li></ul>");
    }

    #[test]
    fn sanitize_fields_mixed() {
        let mut fields = HashMap::new();
        fields.insert("text".into(), serde_json::json!("**bold**"));
        fields.insert("list".into(), serde_json::json!(["a", "b"]));
        let result = sanitize_fields(&fields);
        assert!(result["text"].contains("<strong>"));
        assert!(result["list"].contains("<li>"));
    }
}
