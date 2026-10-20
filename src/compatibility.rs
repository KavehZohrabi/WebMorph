//! Evidence-based assessment of whether the lightweight HTML result contains
//! usable content, a likely JavaScript shell, or a restricted challenge page.

use crate::dom::{ContentKind, ContentNode, Document};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageAssessment {
    Useful,
    Empty,
    JavaScriptShell,
    Restricted,
}

pub fn assess(source: &str, document: &Document) -> PageAssessment {
    let lowered = source.to_ascii_lowercase();
    let mut visible_text = document.title.to_ascii_lowercase();
    for node in &document.content {
        append_text(node, &mut visible_text);
    }
    if [
        "verify you are human",
        "complete the captcha",
        "captcha challenge",
        "human verification",
        "checking your browser",
        "just a moment",
        "attention required",
        "access denied",
        "automated requests",
        "unusual traffic",
    ]
    .iter()
    .any(|marker| visible_text.contains(marker))
        || ["cf-chl", "challenge-platform", "recaptcha", "hcaptcha"]
            .iter()
            .any(|marker| lowered.contains(marker))
    {
        return PageAssessment::Restricted;
    }

    let mut visible_chars = 0usize;
    let mut visible_nodes = 0usize;
    for node in &document.content {
        count_text(node, &mut visible_chars, &mut visible_nodes);
    }
    if visible_chars >= 40 || !document.interactive.is_empty() || !document.forms.is_empty() {
        return PageAssessment::Useful;
    }

    let has_shell_evidence = [
        "<script",
        "type=\"module\"",
        "id=\"root\"",
        "id=\"app\"",
        "id=\"__next\"",
        "id=\"__nuxt\"",
        "data-reactroot",
        "ng-app",
    ]
    .iter()
    .any(|marker| lowered.contains(marker));
    if has_shell_evidence && (visible_chars < 40 || visible_nodes == 0) {
        PageAssessment::JavaScriptShell
    } else if visible_chars == 0 {
        PageAssessment::Empty
    } else {
        PageAssessment::Useful
    }
}

fn count_text(node: &ContentNode, chars: &mut usize, nodes: &mut usize) {
    let text = match &node.kind {
        ContentKind::Heading { text, .. }
        | ContentKind::Paragraph { text }
        | ContentKind::ListItem { text }
        | ContentKind::TableCell { text, .. }
        | ContentKind::Fallback { text, .. }
        | ContentKind::Text { text }
        | ContentKind::Link { text, .. } => text,
        _ => "",
    };
    let prefer_children = matches!(
        &node.kind,
        ContentKind::Paragraph { .. } | ContentKind::ListItem { .. }
    ) && !node.children.is_empty();
    if !prefer_children && !text.trim().is_empty() {
        *chars = chars.saturating_add(text.chars().count());
        *nodes = nodes.saturating_add(1);
    }
    for child in &node.children {
        count_text(child, chars, nodes);
    }
}

fn append_text(node: &ContentNode, output: &mut String) {
    let text = match &node.kind {
        ContentKind::Heading { text, .. }
        | ContentKind::Paragraph { text }
        | ContentKind::ListItem { text }
        | ContentKind::TableCell { text, .. }
        | ContentKind::Fallback { text, .. }
        | ContentKind::Text { text }
        | ContentKind::Link { text, .. } => text,
        _ => "",
    };
    output.push_str(text);
    output.push(' ');
    for child in &node.children {
        append_text(child, output);
    }
}

#[cfg(test)]
mod tests {
    use super::{PageAssessment, assess};
    use crate::html;

    #[test]
    fn distinguishes_useful_empty_shell_and_restricted_pages() {
        let url = "https://example.test/";
        let useful_source = "<main><h1>Guide</h1><p>This documentation page has enough visible content to be useful.</p></main>";
        assert_eq!(
            assess(useful_source, &html::parse(url, useful_source)),
            PageAssessment::Useful
        );
        let shell = "<div id=\"root\"></div><script type=\"module\" src=\"/app.js\"></script>";
        assert_eq!(
            assess(shell, &html::parse(url, shell)),
            PageAssessment::JavaScriptShell
        );
        assert_eq!(
            assess(
                "<html><body></body></html>",
                &html::parse(url, "<html><body></body></html>")
            ),
            PageAssessment::Empty
        );
        let challenge = "<title>Access denied</title><p>Verify you are human</p>";
        assert_eq!(
            assess(challenge, &html::parse(url, challenge)),
            PageAssessment::Restricted
        );
    }
}
