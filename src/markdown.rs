use std::collections::{HashMap, HashSet};

use ammonia::{Builder, UrlRelative};
use comrak::{markdown_to_html, Options};

/// Strict allowlist for user-authored text:
/// no images, no tables, no raw HTML passthrough, no headings above h3 (a
/// user-authored h1/h2 would fight the page's own heading).
const ALLOWED_TAGS: &[&str] = &[
    "p",
    "br",
    "strong",
    "em",
    "code",
    "pre",
    "ul",
    "ol",
    "li",
    "blockquote",
    "a",
    "h3",
    "h4",
    "h5",
    "h6",
];

/// Renders markdown (procedure bodies/descriptions, comments, thread notes,
/// profile bios) to sanitized HTML for browser display. Every table storing
/// this kind of content keeps the raw markdown as its source of truth --
/// this is only ever called on read (or to populate a cache), never the
/// only copy kept. Agents asking for `text/plain` get the raw markdown
/// directly instead, unrendered -- see the content-negotiation helpers in
/// handlers/mod.rs.
pub fn render_markdown(raw: &str) -> String {
    let unsafe_html = markdown_to_html(raw, &Options::default());
    sanitize(&unsafe_html)
}

fn sanitize(unsafe_html: &str) -> String {
    let tags: HashSet<&str> = ALLOWED_TAGS.iter().copied().collect();
    let mut tag_attributes: HashMap<&str, HashSet<&str>> = HashMap::new();
    tag_attributes.insert("a", HashSet::from(["href"]));

    Builder::default()
        .tags(tags)
        // Empty, not the default {"lang", "title"} -- nothing but `a`'s
        // `href` is allowed anywhere, per the strict allowlist.
        .generic_attributes(HashSet::new())
        .tag_attributes(tag_attributes)
        .url_schemes(HashSet::from(["http", "https"]))
        // Reject relative/schemeless URLs outright rather than passing them
        // through -- "only http/https schemes" means exactly that.
        .url_relative(UrlRelative::Deny)
        .link_rel(Some("nofollow noopener"))
        .set_tag_attribute_value("a", "target", "_blank")
        .clean(unsafe_html)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_basic_formatting() {
        let html = render_markdown("**bold** and _em_ and `code`");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>em</em>"));
        assert!(html.contains("<code>code</code>"));
    }

    #[test]
    fn links_get_nofollow_noopener_and_blank_target() {
        let html = render_markdown("[air](https://airo-bot.com)");
        assert!(html.contains(r#"href="https://airo-bot.com""#));
        assert!(html.contains(r#"rel="nofollow noopener""#));
        assert!(html.contains(r#"target="_blank""#));
    }

    #[test]
    fn drops_disallowed_tags_but_keeps_their_text() {
        let html = render_markdown("# Heading one\n\n## Heading two\n\nplain");
        assert!(!html.contains("<h1"));
        assert!(!html.contains("<h2"));
        assert!(html.contains("Heading one"));
        assert!(html.contains("Heading two"));
    }

    #[test]
    fn strips_images() {
        let html = render_markdown("![alt](https://example.com/x.png)");
        assert!(!html.contains("<img"));
    }

    #[test]
    fn no_raw_html_passthrough() {
        let html = render_markdown("plain <div class=\"x\">raw html</div> text");
        assert!(!html.contains("<div"));
        assert!(html.contains("raw html"));
    }

    /// The exact scenario requested: a comment containing a script tag, an
    /// onerror attribute, and a javascript: link must render harmless.
    /// "Harmless" means nothing executable survives -- comrak itself
    /// already neutralizes raw HTML into a comment ammonia then strips, so
    /// the literal payload text (e.g. "alert(...)") may still appear as
    /// inert visible text; that's fine, it just can't run.
    #[test]
    fn sanitizes_script_tags_event_handlers_and_javascript_links() {
        let raw = concat!(
            "Hi <script>alert('xss')</script>\n\n",
            "<img src=x onerror=\"alert('xss')\">\n\n",
            "[click me](javascript:alert('xss'))",
        );
        let html = render_markdown(raw);

        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
        assert!(!html.to_lowercase().contains("onerror="));
        assert!(!html.to_lowercase().contains("javascript:"));
    }

    #[test]
    fn only_http_and_https_link_schemes_survive() {
        let html = render_markdown("[a](https://example.com) [b](ftp://example.com/file)");
        assert!(html.contains(r#"href="https://example.com""#));
        assert!(!html.contains("ftp://"));
    }
}
