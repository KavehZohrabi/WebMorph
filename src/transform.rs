use crate::dom::{
    ContentBlock, ContentKind, ContentNode, Document, FormDefinition, InteractiveElement,
    InteractiveKind,
};
use std::collections::HashMap;
use unicode_width::UnicodeWidthChar;

/// A renderer-neutral semantic node. Terminal renderers and future browser-backed
/// renderers can consume this tree without depending on one another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub kind: ComponentKind,
    pub children: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentKind {
    Page {
        title: String,
        url: String,
    },
    Group,
    TextBlock {
        text: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    Link {
        text: String,
        target: Option<String>,
        interactive_id: Option<String>,
    },
    List {
        ordered: bool,
    },
    ListItem {
        text: String,
    },
    Navigation {
        label: Option<String>,
    },
    Table,
    TableRow,
    TableCell {
        header: bool,
        text: String,
    },
    Form {
        name: Option<String>,
        action: Option<String>,
        method: String,
    },
    Input {
        interactive_id: String,
        label: String,
        input_type: Option<String>,
        placeholder: Option<String>,
        value: Option<String>,
        disabled: bool,
    },
    Button {
        interactive_id: String,
        label: String,
        disabled: bool,
    },
    InfoPanel {
        title: String,
        text: String,
    },
    StatusIndicator {
        label: String,
        state: StatusState,
    },
    Fallback {
        tag: String,
        summary: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusState {
    Info,
    Warning,
    Error,
    Success,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutHints {
    pub terminal_width: usize,
    pub content_width: usize,
    pub compact: bool,
    pub table_mode: TableMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableMode {
    Columns,
    Stacked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiPage {
    pub title: String,
    pub root: Component,
    pub layout: LayoutHints,
    pub lines: Vec<String>,
    pub forms: Vec<FormDefinition>,
    /// Built-in plugin that adapted the page, if any.
    pub plugin_name: Option<String>,
    /// Safe navigation shortcuts contributed by the selected plugin.
    pub plugin_navigation: Vec<crate::plugins::NavigationAction>,
}

/// Transform using a conventional 80-column viewport.
pub fn transform(document: &Document) -> TuiPage {
    transform_for_width(document, 80)
}

/// Build the semantic component tree and terminal layout for a given width.
pub fn transform_for_width(document: &Document, width: usize) -> TuiPage {
    let width = width.max(1);
    let compact = width < 60;
    let layout = LayoutHints {
        terminal_width: width,
        content_width: width.saturating_sub(if width > 2 { 2 } else { 0 }).max(1),
        compact,
        table_mode: if width < 72 {
            TableMode::Stacked
        } else {
            TableMode::Columns
        },
    };
    let mut children = Vec::new();
    if let Some(description) = &document.metadata.description {
        children.push(Component {
            kind: ComponentKind::InfoPanel {
                title: "About this page".into(),
                text: description.clone(),
            },
            children: vec![],
        });
    }
    if document.content.is_empty() {
        for block in &document.blocks {
            let kind = match block {
                ContentBlock::Heading { level, text } => ComponentKind::Heading {
                    level: *level,
                    text: text.clone(),
                },
                ContentBlock::Paragraph { text } => ComponentKind::TextBlock { text: text.clone() },
                ContentBlock::Link { text, href } => ComponentKind::Link {
                    text: text.clone(),
                    target: Some(href.clone()),
                    interactive_id: None,
                },
            };
            children.push(Component {
                kind,
                children: vec![],
            });
        }
    } else {
        let interactive: HashMap<&str, &InteractiveElement> = document
            .interactive
            .iter()
            .filter(|item| matches!(item.kind, InteractiveKind::Input | InteractiveKind::Button))
            .map(|item| (item.id.as_str(), item))
            .collect();
        for node in &document.content {
            children.push(map_node(node, &interactive));
        }
    }
    if children.is_empty() {
        children.push(Component {
            kind: ComponentKind::InfoPanel {
                title: "No readable page content".into(),
                text: "The server returned HTML, but WebMorph could not find visible text or supported controls. The page may require JavaScript or use an unsupported layout.".into(),
            },
            children: vec![],
        });
    }
    if !document.unsupported_elements.is_empty() {
        let count = document.unsupported_elements.len();
        children.push(Component {
            kind: ComponentKind::StatusIndicator {
                label: format!(
                    "{count} unsupported element{} shown with fallback",
                    if count == 1 { "" } else { "s" }
                ),
                state: StatusState::Warning,
            },
            children: vec![],
        });
    }
    let root = Component {
        kind: ComponentKind::Page {
            title: document.title.clone(),
            url: document.url.clone(),
        },
        children,
    };
    let lines = layout_components(&root, width);
    TuiPage {
        title: document.title.clone(),
        root,
        layout,
        lines,
        forms: document.forms.clone(),
        plugin_name: None,
        plugin_navigation: Vec::new(),
    }
}

/// Lay out an existing component tree for a concrete viewport width. This
/// returns plain lines and has no dependency on the terminal renderer.
pub fn layout_components(root: &Component, width: usize) -> Vec<String> {
    let width = width.max(1);
    let hints = LayoutHints {
        terminal_width: width,
        content_width: width.saturating_sub(if width > 2 { 2 } else { 0 }).max(1),
        compact: width < 60,
        table_mode: if width < 72 {
            TableMode::Stacked
        } else {
            TableMode::Columns
        },
    };
    let mut lines = Vec::new();
    for child in &root.children {
        render(child, hints, 0, &mut lines);
    }
    for line in &mut lines {
        *line = truncate_line(line, hints.content_width);
        strip_terminal_controls_in_place(line);
    }
    lines
}

/// Strip terminal control characters from text originating outside the UI.
/// Ratatui writes text to the terminal as-is, so an embedded ESC could otherwise
/// be interpreted as a terminal command by some terminal emulators.
pub fn strip_terminal_controls(text: &str) -> String {
    if !has_terminal_controls(text) {
        return text.to_owned();
    }
    text.chars().filter(|ch| !ch.is_control()).collect()
}

/// Sanitize an owned display string without allocating a second copy.
pub fn strip_terminal_controls_in_place(text: &mut String) {
    if has_terminal_controls(text) {
        text.retain(|ch| !ch.is_control());
    }
}

fn has_terminal_controls(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        *byte < 0x20
            || *byte == 0x7f
            || (*byte == 0xc2
                && bytes
                    .get(index + 1)
                    .is_some_and(|next| (0x80..=0x9f).contains(next)))
    })
}

fn map_node(node: &ContentNode, interactive: &HashMap<&str, &InteractiveElement>) -> Component {
    let children = node
        .children
        .iter()
        .map(|child| map_node(child, interactive))
        .collect();
    let kind = match &node.kind {
        ContentKind::Heading { level, text } => ComponentKind::Heading {
            level: *level,
            text: text.clone(),
        },
        ContentKind::Paragraph { text } => ComponentKind::TextBlock { text: text.clone() },
        ContentKind::Link {
            text,
            href,
            interactive_id,
        } => ComponentKind::Link {
            text: text.clone(),
            target: href.clone(),
            interactive_id: interactive_id.clone(),
        },
        ContentKind::Image { src, alt, .. } => ComponentKind::InfoPanel {
            title: "Image".into(),
            text: if alt.is_empty() {
                src.clone().unwrap_or_else(|| "Image unavailable".into())
            } else {
                format!(
                    "{alt}{}",
                    src.as_ref().map(|s| format!(" ({s})")).unwrap_or_default()
                )
            },
        },
        ContentKind::List { ordered } => ComponentKind::List { ordered: *ordered },
        ContentKind::ListItem { text } => ComponentKind::ListItem { text: text.clone() },
        ContentKind::Table | ContentKind::TableRow => {
            if matches!(&node.kind, ContentKind::Table) {
                ComponentKind::Table
            } else {
                ComponentKind::TableRow
            }
        }
        ContentKind::TableCell { header, text } => ComponentKind::TableCell {
            header: *header,
            text: text.clone(),
        },
        ContentKind::Form {
            name,
            action,
            method,
        } => ComponentKind::Form {
            name: name.clone(),
            action: action.clone(),
            method: method.clone(),
        },
        ContentKind::Input { interactive_id } => {
            let item = interactive.get(interactive_id.as_str()).copied();
            ComponentKind::Input {
                interactive_id: interactive_id.clone(),
                label: item.map(|i| i.label.clone()).unwrap_or_default(),
                input_type: item.and_then(|i| i.input_type.clone()),
                placeholder: item.and_then(|i| i.placeholder.clone()),
                value: item.and_then(|i| i.value.clone()),
                disabled: item.is_some_and(|i| i.disabled),
            }
        }
        ContentKind::Button { interactive_id } => {
            let item = interactive.get(interactive_id.as_str()).copied();
            ComponentKind::Button {
                interactive_id: interactive_id.clone(),
                label: item.map(|i| i.label.clone()).unwrap_or_default(),
                disabled: item.is_some_and(|i| i.disabled),
            }
        }
        ContentKind::Navigation { label } => ComponentKind::Navigation {
            label: label.clone(),
        },
        ContentKind::Text { text } if !text.is_empty() => {
            ComponentKind::TextBlock { text: text.clone() }
        }
        ContentKind::Text { .. } => ComponentKind::Group,
        ContentKind::Fallback { tag, text } => ComponentKind::Fallback {
            tag: tag.clone(),
            summary: text.clone(),
        },
    };
    Component { kind, children }
}

fn render(component: &Component, hints: LayoutHints, depth: usize, out: &mut Vec<String>) {
    let indent = "  ".repeat(depth.min(8));
    match &component.kind {
        ComponentKind::Page { .. } | ComponentKind::Group | ComponentKind::TableRow => {
            for child in &component.children {
                render(child, hints, depth, out);
            }
        }
        ComponentKind::Heading { level, text } => {
            out.push(String::new());
            push_wrapped(
                out,
                &format!("{} {text}", "#".repeat((*level as usize).clamp(1, 6))),
                hints.content_width,
                &indent,
            );
        }
        ComponentKind::TextBlock { text } => {
            push_wrapped(out, text, hints.content_width, &indent);
            for child in &component.children {
                if matches!(child.kind, ComponentKind::Link { .. }) {
                    render(child, hints, depth + 1, out);
                }
            }
        }
        ComponentKind::Link {
            text,
            target,
            interactive_id,
        } => {
            let label = if text.trim().is_empty() {
                target.as_deref().unwrap_or("Link")
            } else {
                text
            };
            let marker = interactive_id
                .as_deref()
                .map(|id| format!(" [{id}]"))
                .unwrap_or_default();
            push_wrapped(
                out,
                &format!("↗ {label}{marker}"),
                hints.content_width,
                &indent,
            );
        }
        ComponentKind::List { ordered } => {
            for (index, child) in component.children.iter().enumerate() {
                let prefix = if *ordered {
                    format!("{}. ", index + 1)
                } else {
                    "• ".into()
                };
                render_list_child(child, hints, depth, &prefix, out);
            }
        }
        ComponentKind::ListItem { text } => {
            push_wrapped(out, &format!("• {text}"), hints.content_width, &indent)
        }
        ComponentKind::Navigation { label } => {
            if let Some(label) = label.as_ref().filter(|s| !s.is_empty()) {
                push_wrapped(
                    out,
                    &format!("Navigation: {label}"),
                    hints.content_width,
                    &indent,
                );
            }
            let mut links = Vec::new();
            collect_nav_links(component, &mut links);
            if !links.is_empty() {
                let separator = if hints.compact { "\n" } else { "  •  " };
                push_wrapped(out, &links.join(separator), hints.content_width, &indent);
            } else {
                for child in &component.children {
                    render(child, hints, depth + 1, out);
                }
            }
        }
        ComponentKind::Table => render_table(component, hints, depth, out),
        ComponentKind::TableCell { .. } => {}
        ComponentKind::Form {
            name,
            action,
            method,
        } => {
            let title = name.as_deref().filter(|s| !s.is_empty()).unwrap_or("Form");
            out.push(format!(
                "┌─ {title} ({}){}",
                method.to_uppercase(),
                action
                    .as_deref()
                    .map(|s| format!(" → {s}"))
                    .unwrap_or_default()
            ));
            for child in &component.children {
                render(child, hints, depth + 1, out);
            }
            out.push("└".into());
        }
        ComponentKind::Input {
            interactive_id,
            label,
            input_type,
            placeholder,
            value,
            disabled,
        } => {
            if input_type.as_deref() == Some("hidden") {
                return;
            }
            let descriptor = if !label.is_empty() {
                label.clone()
            } else {
                placeholder
                    .clone()
                    .unwrap_or_else(|| input_type.clone().unwrap_or_else(|| "input".into()))
            };
            let value = value
                .as_deref()
                .map(|v| format!(" = {v}"))
                .unwrap_or_default();
            push_wrapped(
                out,
                &format!(
                    "[ ] {descriptor}{value} [{interactive_id}]{}",
                    if *disabled { " (disabled)" } else { "" }
                ),
                hints.content_width,
                &indent,
            );
        }
        ComponentKind::Button {
            interactive_id,
            label,
            disabled,
        } => push_wrapped(
            out,
            &format!(
                "[ {label} ] [{interactive_id}]{}",
                if *disabled { " (disabled)" } else { "" }
            ),
            hints.content_width,
            &indent,
        ),
        ComponentKind::InfoPanel { title, text } => {
            out.push(format!(
                "┌─ {title} {}",
                "─".repeat(
                    hints
                        .content_width
                        .saturating_sub(title.chars().count() + 5)
                        .min(24)
                )
            ));
            push_wrapped(
                out,
                text,
                hints.content_width.saturating_sub(2).max(1),
                "│ ",
            );
            out.push("└".into());
        }
        ComponentKind::StatusIndicator { label, state } => {
            let icon = match state {
                StatusState::Info => "i",
                StatusState::Warning => "!",
                StatusState::Error => "×",
                StatusState::Success => "✓",
            };
            push_wrapped(
                out,
                &format!("[{icon}] {label}"),
                hints.content_width,
                &indent,
            );
        }
        ComponentKind::Fallback { tag, summary } => {
            let summary = if summary.is_empty() {
                "Content is available in a simplified fallback."
            } else {
                summary
            };
            push_wrapped(
                out,
                &format!("[Unsupported {tag}] {summary}"),
                hints.content_width,
                &indent,
            );
        }
    }
}

fn render_list_child(
    child: &Component,
    hints: LayoutHints,
    depth: usize,
    prefix: &str,
    out: &mut Vec<String>,
) {
    if let ComponentKind::ListItem { text } = &child.kind {
        push_wrapped(
            out,
            &format!("{prefix}{text}"),
            hints.content_width,
            &"  ".repeat(depth.min(8)),
        );
        for nested in &child.children {
            if let ComponentKind::Link {
                text,
                interactive_id,
                ..
            } = &nested.kind
            {
                let id = interactive_id
                    .as_deref()
                    .map(|id| format!(" [{id}]"))
                    .unwrap_or_default();
                push_wrapped(
                    out,
                    &format!("  ↳ {text}{id}"),
                    hints.content_width,
                    &"  ".repeat((depth + 1).min(8)),
                );
            }
        }
    } else {
        render(child, hints, depth, out);
    }
}

fn render_table(table: &Component, hints: LayoutHints, depth: usize, out: &mut Vec<String>) {
    let mut rows = Vec::new();
    collect_table_rows(table, &mut rows);
    let headers = rows
        .iter()
        .find_map(|r| {
            let found = r
                .children
                .iter()
                .filter_map(|c| match &c.kind {
                    ComponentKind::TableCell { header: true, text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (!found.is_empty()).then_some(found)
        })
        .unwrap_or_default();
    for row in rows {
        let cells = row
            .children
            .iter()
            .filter_map(|cell| match &cell.kind {
                ComponentKind::TableCell { header, text } => Some((*header, text.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        if cells.is_empty() {
            continue;
        }
        if hints.table_mode == TableMode::Columns {
            push_wrapped(
                out,
                &cells
                    .iter()
                    .map(|(_, text)| *text)
                    .collect::<Vec<_>>()
                    .join(" │ "),
                hints.content_width,
                &"  ".repeat(depth.min(8)),
            );
        } else if cells.iter().any(|(header, _)| *header) {
            for (_, text) in cells {
                push_wrapped(
                    out,
                    text,
                    hints.content_width.saturating_sub(2).max(1),
                    "  ",
                );
            }
        } else {
            for (index, (_, text)) in cells.iter().enumerate() {
                let label = headers.get(index).copied().unwrap_or("Value");
                push_wrapped(
                    out,
                    &format!("{label}: {text}"),
                    hints.content_width,
                    &"  ".repeat(depth.min(8)),
                );
            }
        }
    }
}

fn collect_table_rows<'a>(component: &'a Component, rows: &mut Vec<&'a Component>) {
    for child in &component.children {
        if matches!(child.kind, ComponentKind::TableRow) {
            rows.push(child);
        } else {
            collect_table_rows(child, rows);
        }
    }
}

fn collect_nav_links(component: &Component, links: &mut Vec<String>) {
    for child in &component.children {
        if let ComponentKind::Link {
            text,
            interactive_id,
            ..
        } = &child.kind
        {
            let marker = interactive_id
                .as_deref()
                .map(|id| format!(" [{id}]"))
                .unwrap_or_default();
            links.push(format!("↗ {text}{marker}"));
        } else {
            collect_nav_links(child, links);
        }
    }
}

/// Reusable word wrapping primitive; oversized words are split on character boundaries.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for word in text.split_whitespace() {
        let word_width = display_width(word);
        if used > 0 && used + 1 + word_width <= width {
            line.push(' ');
            line.push_str(word);
            used += 1 + word_width;
        } else if used == 0 && word_width <= width {
            line.push_str(word);
            used = word_width;
        } else {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            for ch in word.chars() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if used > 0 && used + cw > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(ch);
                used += cw;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn push_wrapped(out: &mut Vec<String>, text: &str, width: usize, prefix: &str) {
    let continuation = " ".repeat(display_width(prefix));
    for (index, line) in wrap_text(text, width.saturating_sub(display_width(prefix)).max(1))
        .into_iter()
        .enumerate()
    {
        out.push(format!(
            "{}{line}",
            if index == 0 { prefix } else { &continuation }
        ));
    }
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

fn truncate_line(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let ellipsis = "…";
    let available = width.saturating_sub(display_width(ellipsis));
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > available {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push_str(ellipsis);
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Component, ComponentKind, TableMode, layout_components, transform, transform_for_width,
        wrap_text,
    };
    use crate::{dom::Document, html};

    #[test]
    fn builds_semantic_tree_and_keeps_interactive_ids() {
        let doc = html::parse(
            "https://example.com",
            "<h1>Docs</h1><nav><a href='/start'>Start</a></nav><ul><li>One</li></ul><form><input name='q' placeholder='Search'><button>Go</button></form>",
        );
        let page = transform(&doc);
        assert!(matches!(page.root.kind, ComponentKind::Page { .. }));
        assert!(
            page.root
                .children
                .iter()
                .any(|c| matches!(c.kind, ComponentKind::Heading { level: 1, .. }))
        );
        let mut ids = Vec::new();
        collect_ids(&page.root, &mut ids);
        assert!(ids.contains(&"wm-0001".to_owned()));
        assert!(ids.contains(&"wm-0002".to_owned()));
        assert!(ids.contains(&"wm-0003".to_owned()));
    }

    fn collect_ids(component: &super::Component, ids: &mut Vec<String>) {
        match &component.kind {
            ComponentKind::Link {
                interactive_id: Some(id),
                ..
            }
            | ComponentKind::Input {
                interactive_id: id, ..
            }
            | ComponentKind::Button {
                interactive_id: id, ..
            } => ids.push(id.clone()),
            _ => {}
        }
        for child in &component.children {
            collect_ids(child, ids);
        }
    }

    #[test]
    fn adapts_layout_to_narrow_and_wide_terminals() {
        let doc = html::parse(
            "https://example.com",
            "<table><tr><th>Name</th><th>Count</th></tr><tr><td>Alpha</td><td>123</td></tr></table><p>A very long paragraph with enough words to wrap across a narrow terminal width safely.</p><a href='/an-extremely-long-destination-that-never-fits'>A long link label</a>",
        );
        let narrow = transform_for_width(&doc, 24);
        let wide = transform_for_width(&doc, 100);
        assert_eq!(narrow.layout.table_mode, TableMode::Stacked);
        assert_eq!(wide.layout.table_mode, TableMode::Columns);
        assert!(
            narrow
                .lines
                .iter()
                .all(|line| super::display_width(line) <= narrow.layout.content_width)
        );
        assert!(narrow.lines.len() > wide.lines.len());
    }

    #[test]
    fn removes_terminal_control_sequences_from_untrusted_component_text() {
        let root = Component {
            kind: ComponentKind::Page {
                title: "t".into(),
                url: "u".into(),
            },
            children: vec![Component {
                kind: ComponentKind::TextBlock {
                    text: "safe\u{1b}[2J text\u{009b}31m".into(),
                },
                children: vec![],
            }],
        };
        let lines = layout_components(&root, 80);
        assert_eq!(lines, ["safe[2J text31m"]);
    }

    #[test]
    fn tiny_widths_and_wide_glyphs_wrap_safely() {
        assert_eq!(wrap_text("界a", 1), vec!["界", "a"]);
        let page = transform_for_width(&Document::new("x", "Tiny"), 0);
        assert_eq!(page.layout.terminal_width, 1);
    }

    #[test]
    fn flat_legacy_documents_still_transform() {
        let mut doc = Document::new("https://example.com", "Legacy");
        doc.blocks.push(crate::dom::ContentBlock::Paragraph {
            text: "legacy text".into(),
        });
        assert_eq!(transform(&doc).lines, vec!["legacy text"]);
    }
}
