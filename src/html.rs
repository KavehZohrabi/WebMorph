use reqwest::Url;
use scraper::{ElementRef, Html, Selector};
use std::{
    collections::{HashMap, VecDeque},
    sync::OnceLock,
};

use crate::dom::{
    ContentBlock, ContentKind, ContentNode, Document, FormControl, FormDefinition,
    InteractiveElement, InteractiveKind,
};

const IGNORED: &[&str] = &[
    "script", "style", "noscript", "template", "head", "meta", "link", "title",
];

pub fn parse(url: &str, source: &str) -> Document {
    let html = Html::parse_document(source);
    let title = select_first(&html, "title")
        .map(text_of)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| url.to_owned());
    let mut document = Document::new(url, title);
    let document_url = Url::parse(url).ok();
    let base = select_first(&html, "base[href]")
        .and_then(|e| resolve(document_url.as_ref(), e.value().attr("href")))
        .and_then(|s| Url::parse(&s).ok())
        .or_else(|| document_url.clone());
    document.metadata.base_url = base.as_ref().map(ToString::to_string);
    document.metadata.language =
        select_first(&html, "html[lang]").and_then(|e| e.value().attr("lang").map(str::to_owned));
    document.metadata.description = select_first(&html, "meta[name='description']").and_then(|e| {
        e.value()
            .attr("content")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    });

    let mut ordinal = 0usize;
    if let Some(body) = select_first(&html, "body") {
        document.content = walk_children(body, base.as_ref(), &mut document, &mut ordinal);
    } else if let Some(root) = select_first(&html, "html") {
        document.content = walk_children(root, base.as_ref(), &mut document, &mut ordinal);
    }
    document.forms = parse_forms(&html, &mut document, base.as_ref());
    // Keep the original flat API usable by the existing renderer and plugins.
    collect_legacy_blocks(&document.content, &mut document.blocks);
    document
}

fn select_first<'a>(html: &'a Html, selector: &str) -> Option<ElementRef<'a>> {
    html.select(cached_selector(selector)).next()
}

fn cached_selector(selector: &str) -> &'static Selector {
    static SELECTORS: OnceLock<HashMap<&'static str, Selector>> = OnceLock::new();
    let selectors = SELECTORS.get_or_init(|| {
        [
            "title",
            "body",
            "html",
            "base[href]",
            "html[lang]",
            "meta[name='description']",
            "form",
            "input, textarea, select, button",
            "option",
            "option[selected]",
            "label[for]",
        ]
        .into_iter()
        .filter_map(|value| Selector::parse(value).ok().map(|parsed| (value, parsed)))
        .collect()
    });
    selectors.get(selector).expect("known static HTML selector")
}

fn walk_children(
    parent: ElementRef<'_>,
    base: Option<&Url>,
    document: &mut Document,
    ordinal: &mut usize,
) -> Vec<ContentNode> {
    let mut out = Vec::new();
    for child in parent.children() {
        if let Some(text) = child.value().as_text() {
            let value = normalize(text);
            if !value.is_empty() {
                out.push(ContentNode {
                    kind: ContentKind::Text { text: value },
                    children: vec![],
                });
            }
        } else if let Some(element) = ElementRef::wrap(child)
            && let Some(node) = parse_element(element, base, document, ordinal)
        {
            out.push(node);
        }
    }
    out
}

fn parse_element(
    element: ElementRef<'_>,
    base: Option<&Url>,
    document: &mut Document,
    ordinal: &mut usize,
) -> Option<ContentNode> {
    let tag = element.value().name();
    if IGNORED.contains(&tag)
        || element.value().attr("hidden").is_some()
        || element.value().attr("aria-hidden") == Some("true")
    {
        return None;
    }
    let leaf = |kind| {
        Some(ContentNode {
            kind,
            children: vec![],
        })
    };
    match tag {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => leaf(ContentKind::Heading {
            level: tag[1..].parse().unwrap_or(1),
            text: text_of(element),
        }),
        "p" => Some(ContentNode {
            kind: ContentKind::Paragraph {
                text: text_of(element),
            },
            children: walk_children(element, base, document, ordinal),
        }),
        "a" => {
            let href = resolve(base, element.value().attr("href"));
            let text = text_of(element);
            let id = href.as_ref().map(|_| {
                add_interactive(
                    document,
                    ordinal,
                    InteractiveKind::Link,
                    text.clone(),
                    None,
                    None,
                    None,
                    href.clone(),
                    None,
                    None,
                    element.value().attr("aria-disabled") == Some("true"),
                    element.value().attr("id").map(str::to_owned),
                )
            });
            leaf(ContentKind::Link {
                text,
                href,
                interactive_id: id,
            })
        }
        "img" => {
            let alt = element.value().attr("alt").unwrap_or("").trim().to_owned();
            let src = resolve(base, element.value().attr("src"));
            leaf(ContentKind::Image {
                src,
                alt,
                interactive_id: None,
            })
        }
        "ul" | "ol" => Some(ContentNode {
            kind: ContentKind::List {
                ordered: tag == "ol",
            },
            children: walk_children(element, base, document, ordinal),
        }),
        "li" => Some(ContentNode {
            kind: ContentKind::ListItem {
                text: text_of(element),
            },
            children: walk_children(element, base, document, ordinal),
        }),
        "table" => Some(ContentNode {
            kind: ContentKind::Table,
            children: walk_children(element, base, document, ordinal),
        }),
        "thead" | "tbody" | "tfoot" => Some(ContentNode {
            kind: ContentKind::Text {
                text: String::new(),
            },
            children: walk_children(element, base, document, ordinal),
        }),
        "tr" => Some(ContentNode {
            kind: ContentKind::TableRow,
            children: walk_children(element, base, document, ordinal),
        }),
        "th" | "td" => leaf(ContentKind::TableCell {
            header: tag == "th",
            text: text_of(element),
        }),
        "form" => Some(ContentNode {
            kind: ContentKind::Form {
                name: element.value().attr("name").map(str::to_owned),
                action: resolve(base, element.value().attr("action")),
                method: element
                    .value()
                    .attr("method")
                    .unwrap_or("get")
                    .to_ascii_lowercase(),
            },
            children: walk_children(element, base, document, ordinal),
        }),
        "input" | "textarea" | "select" => {
            let input_type = if tag == "input" {
                element.value().attr("type").unwrap_or("text")
            } else {
                tag
            };
            let label = element
                .value()
                .attr("aria-label")
                .or_else(|| element.value().attr("placeholder"))
                .or_else(|| element.value().attr("name"))
                .unwrap_or("")
                .to_owned();
            let id = add_interactive(
                document,
                ordinal,
                InteractiveKind::Input,
                label,
                element.value().attr("name").map(str::to_owned),
                if input_type.eq_ignore_ascii_case("password") {
                    None
                } else {
                    element.value().attr("value").map(str::to_owned)
                },
                Some(input_type.to_owned()),
                None,
                None,
                element.value().attr("placeholder").map(str::to_owned),
                element.value().attr("disabled").is_some(),
                element.value().attr("id").map(str::to_owned),
            );
            leaf(ContentKind::Input { interactive_id: id })
        }
        "button" => {
            let text = text_of(element);
            let label = if text.is_empty() {
                element.value().attr("aria-label").unwrap_or("").to_owned()
            } else {
                text
            };
            let id = add_interactive(
                document,
                ordinal,
                InteractiveKind::Button,
                label,
                element.value().attr("name").map(str::to_owned),
                element.value().attr("value").map(str::to_owned),
                Some(element.value().attr("type").unwrap_or("submit").to_owned()),
                None,
                None,
                None,
                element.value().attr("disabled").is_some(),
                element.value().attr("id").map(str::to_owned),
            );
            leaf(ContentKind::Button { interactive_id: id })
        }
        "nav" => Some(ContentNode {
            kind: ContentKind::Navigation {
                label: element.value().attr("aria-label").map(str::to_owned),
            },
            children: walk_children(element, base, document, ordinal),
        }),
        "iframe" | "canvas" | "video" | "audio" | "object" | "embed" => {
            document.unsupported_elements.push(tag.to_owned());
            leaf(ContentKind::Fallback {
                tag: tag.to_owned(),
                text: text_of(element),
            })
        }
        "html" | "body" | "main" | "article" | "section" | "header" | "footer" | "aside"
        | "div" | "span" | "strong" | "em" | "b" | "i" | "label" | "fieldset" | "legend"
        | "option" | "datalist" | "details" | "summary" | "blockquote" | "figure"
        | "figcaption" | "br" | "hr" => {
            let children = walk_children(element, base, document, ordinal);
            if children.is_empty() {
                None
            } else {
                Some(ContentNode {
                    kind: ContentKind::Text {
                        text: String::new(),
                    },
                    children,
                })
            }
        }
        _ => {
            if tag.contains('-') {
                document.unsupported_elements.push(tag.to_owned());
                leaf(ContentKind::Fallback {
                    tag: tag.to_owned(),
                    text: text_of(element),
                })
            } else {
                walk_children(element, base, document, ordinal)
                    .into_iter()
                    .next()
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_interactive(
    document: &mut Document,
    ordinal: &mut usize,
    kind: InteractiveKind,
    label: String,
    name: Option<String>,
    value: Option<String>,
    input_type: Option<String>,
    href: Option<String>,
    form_action: Option<String>,
    placeholder: Option<String>,
    disabled: bool,
    html_id: Option<String>,
) -> String {
    *ordinal += 1;
    let id = format!("wm-{:04}", *ordinal);
    document.interactive.push(InteractiveElement {
        id: id.clone(),
        kind,
        label,
        name,
        value,
        input_type,
        href,
        form_action,
        placeholder,
        disabled,
        html_id,
    });
    id
}

fn parse_forms(html: &Html, document: &mut Document, base: Option<&Url>) -> Vec<FormDefinition> {
    let form_selector = cached_selector("form");
    let control_selector = cached_selector("input, textarea, select, button");
    let mut forms = Vec::new();
    let mut ids_by_html: HashMap<&str, VecDeque<&str>> = HashMap::new();
    let mut ids_by_name_type: HashMap<&str, HashMap<&str, VecDeque<&str>>> = HashMap::new();
    for item in &document.interactive {
        if !matches!(item.kind, InteractiveKind::Input | InteractiveKind::Button) {
            continue;
        }
        if let Some(id) = &item.html_id {
            ids_by_html
                .entry(id)
                .or_default()
                .push_back(item.id.as_str());
        }
        if let (Some(name), Some(input_type)) = (&item.name, &item.input_type) {
            ids_by_name_type
                .entry(name.as_str())
                .or_default()
                .entry(input_type.as_str())
                .or_default()
                .push_back(item.id.as_str());
        }
    }
    let mut claimed = std::collections::HashSet::new();
    let mut labels = HashMap::new();
    for label in html.select(cached_selector("label[for]")) {
        if let Some(id) = label.value().attr("for") {
            labels
                .entry(id.to_owned())
                .or_insert_with(|| text_of(label));
        }
    }
    for (index, form) in html.select(form_selector).enumerate() {
        let action = resolve(base, form.value().attr("action"))
            .or_else(|| base.map(ToString::to_string))
            .unwrap_or_default();
        let method = form
            .value()
            .attr("method")
            .unwrap_or("get")
            .to_ascii_lowercase();
        let enctype = form
            .value()
            .attr("enctype")
            .unwrap_or("application/x-www-form-urlencoded")
            .to_ascii_lowercase();
        let mut controls = Vec::new();
        for element in form.select(control_selector) {
            if element.value().attr("form").is_some() {
                continue;
            }
            let tag = element.value().name();
            let control_type = match tag {
                "input" => element
                    .value()
                    .attr("type")
                    .unwrap_or("text")
                    .to_ascii_lowercase(),
                "button" => element
                    .value()
                    .attr("type")
                    .unwrap_or("submit")
                    .to_ascii_lowercase(),
                "textarea" => "textarea".into(),
                _ => "select".into(),
            };
            let name = element.value().attr("name").map(str::to_owned);
            let html_id = element.value().attr("id");
            let queue = if let Some(id) = html_id {
                ids_by_html.get_mut(id)
            } else {
                name.as_deref().and_then(|name| {
                    ids_by_name_type
                        .get_mut(name)
                        .and_then(|types| types.get_mut(control_type.as_str()))
                })
            };
            let Some(queue) = queue else {
                continue;
            };
            let interactive_id = loop {
                let Some(candidate) = queue.pop_front() else {
                    break None;
                };
                if claimed.insert(candidate) {
                    break Some(candidate);
                }
            };
            let Some(interactive_id) = interactive_id else {
                continue;
            };
            let options = if tag == "select" {
                let selector = cached_selector("option");
                element
                    .select(selector)
                    .map(|option| {
                        let value = option
                            .value()
                            .attr("value")
                            .map(str::to_owned)
                            .unwrap_or_else(|| text_of(option));
                        (value, text_of(option))
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let value = match tag {
                "textarea" => element.text().collect::<String>().replace("\r\n", "\n"),
                "select" => {
                    let selector = cached_selector("option[selected]");
                    element
                        .select(selector)
                        .next()
                        .map(|option| {
                            option
                                .value()
                                .attr("value")
                                .map(str::to_owned)
                                .unwrap_or_else(|| text_of(option))
                        })
                        .or_else(|| options.first().map(|(value, _)| value.clone()))
                        .unwrap_or_default()
                }
                _ if control_type == "password" => String::new(),
                _ => element
                    .value()
                    .attr("value")
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        if matches!(control_type.as_str(), "checkbox" | "radio") {
                            "on".into()
                        } else {
                            String::new()
                        }
                    }),
            };
            let associated_label = element
                .value()
                .attr("id")
                .and_then(|id| labels.get(id).cloned());
            let label = element
                .value()
                .attr("aria-label")
                .map(str::to_owned)
                .or(associated_label)
                .or_else(|| element.value().attr("placeholder").map(str::to_owned))
                .or_else(|| name.clone())
                .unwrap_or_default();
            controls.push(FormControl {
                interactive_id: interactive_id.to_owned(),
                name,
                control_type,
                value,
                label,
                required: element.value().attr("required").is_some(),
                checked: element.value().attr("checked").is_some()
                    || element.value().attr("selected").is_some(),
                disabled: element.value().attr("disabled").is_some(),
                options,
                multiple: element.value().attr("multiple").is_some(),
                minlength: element
                    .value()
                    .attr("minlength")
                    .and_then(|v| v.parse().ok()),
                maxlength: element
                    .value()
                    .attr("maxlength")
                    .and_then(|v| v.parse().ok()),
            });
        }
        forms.push(FormDefinition {
            id: format!("wm-form-{:04}", index + 1),
            name: form.value().attr("name").map(str::to_owned),
            action,
            method,
            enctype,
            requires_javascript: form.value().attrs().any(|(name, _)| name.starts_with("on"))
                || form.select(control_selector).any(|control| {
                    control
                        .value()
                        .attrs()
                        .any(|(name, _)| name.starts_with("on"))
                }),
            controls,
        });
    }
    forms
}

fn collect_legacy_blocks(nodes: &[ContentNode], out: &mut Vec<ContentBlock>) {
    for node in nodes {
        match &node.kind {
            ContentKind::Heading { level, text } => out.push(ContentBlock::Heading {
                level: *level,
                text: text.clone(),
            }),
            ContentKind::Paragraph { text } => {
                out.push(ContentBlock::Paragraph { text: text.clone() })
            }
            ContentKind::Link {
                text,
                href: Some(href),
                ..
            } if !text.is_empty() => out.push(ContentBlock::Link {
                text: text.clone(),
                href: href.clone(),
            }),
            _ => {}
        }
        collect_legacy_blocks(&node.children, out);
    }
}

fn resolve(base: Option<&Url>, value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.starts_with("javascript:") || value.starts_with("data:") {
        return None;
    }
    let base = base?;
    base.join(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.to_string())
}

fn text_of(element: ElementRef<'_>) -> String {
    normalize_iter(element.text())
}
fn normalize(value: &str) -> String {
    normalize_iter(std::iter::once(value))
}
fn normalize_iter<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts {
        for word in part.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::dom::ContentKind;

    fn gather<'a>(nodes: &'a [crate::dom::ContentNode], kinds: &mut Vec<&'a ContentKind>) {
        for node in nodes {
            kinds.push(&node.kind);
            gather(&node.children, kinds);
        }
    }

    #[test]
    fn parses_simple_page_and_relative_links() {
        let doc = parse(
            "https://example.com/a/page",
            "<title>Demo</title><h1>Hello</h1><p>Readable text <a href='/next'>Next</a></p>",
        );
        assert_eq!(doc.title, "Demo");
        assert_eq!(doc.blocks.len(), 3);
        assert!(doc.blocks.iter().any(|b| matches!(b, crate::dom::ContentBlock::Link { href, .. } if href == "https://example.com/next")));
        assert_eq!(doc.interactive[0].id, "wm-0001");
    }

    #[test]
    fn extracts_nested_structures_controls_navigation_and_metadata() {
        let doc = parse(
            "https://example.com/path",
            "<html lang='en'><head><title>Page</title><meta name='description' content='Info'><base href='/root/'></head><body><nav aria-label='Primary'><ul><li><a href='home'>Home</a></li></ul></nav><form action='submit'><label>Name<input name='q' placeholder='Your name'></label><button>Go</button></form><img src='photo.png' alt='Portrait'></body></html>",
        );
        assert_eq!(doc.metadata.language.as_deref(), Some("en"));
        assert_eq!(doc.metadata.description.as_deref(), Some("Info"));
        let mut kinds = Vec::new();
        gather(&doc.content, &mut kinds);
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, ContentKind::Navigation { .. }))
        );
        assert!(kinds.iter().any(|k| matches!(k, ContentKind::List { .. })));
        assert!(kinds.iter().any(|k| matches!(k, ContentKind::Form { action: Some(action), .. } if action == "https://example.com/root/submit")));
        assert!(kinds.iter().any(|k| matches!(k, ContentKind::Image { alt, src: Some(src), .. } if alt == "Portrait" && src == "https://example.com/root/photo.png")));
        assert_eq!(doc.interactive.len(), 3);
        assert!(doc.interactive.iter().all(|i| i.id.starts_with("wm-")));
    }

    #[test]
    fn parses_tables_and_handles_broken_or_empty_html() {
        let doc = parse(
            "https://example.com",
            "<table><tr><th>A<td>B<tr><td>C<td>D</table><p><b>broken<p>later",
        );
        let mut kinds = Vec::new();
        gather(&doc.content, &mut kinds);
        assert!(kinds.iter().any(|k| matches!(k, ContentKind::Table)));
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, ContentKind::TableCell { header: true, .. }))
        );
        assert!(doc.blocks.len() >= 2);
        let empty = parse("https://example.com/empty", "");
        assert_eq!(empty.title, "https://example.com/empty");
        assert!(empty.content.is_empty());
    }

    #[test]
    fn handles_large_documents_without_losing_content() {
        let mut source = String::from("<main>");
        for n in 0..3000 {
            source.push_str(&format!("<p>paragraph {n}</p>"));
        }
        source.push_str("</main>");
        let doc = parse("https://example.com", &source);
        assert_eq!(doc.blocks.len(), 3000);
        assert_eq!(doc.content.len(), 1);
    }

    #[test]
    fn ignores_scripts_styles_and_falls_back_for_unsupported_elements() {
        let doc = parse(
            "https://example.com",
            "<script>secret()</script><style>.x{}</style><canvas>canvas fallback</canvas><x-widget>widget text</x-widget>",
        );
        assert_eq!(doc.unsupported_elements, vec!["canvas", "x-widget"]);
        assert!(doc.blocks.is_empty());
        assert!(!format!("{:?}", doc.content).contains("secret"));
    }

    #[test]
    fn parses_form_controls_validation_and_relative_actions() {
        let doc = parse(
            "https://example.com/search/index",
            r#"
            <form id="search" action="../results" method="get">
              <input id="q" name="q" type="search" required minlength="2" placeholder="Search">
              <input id="updates" name="updates" type="checkbox" value="yes" checked>
              <select id="sort" name="sort"><option value="new">Newest</option><option selected>Popular</option></select>
              <button id="go" type="submit" name="submit" value="1">Search</button>
            </form>
            <form action="/login" method="post"><input id="password" name="password" type="password"></form>
        "#,
        );
        assert_eq!(doc.forms.len(), 2);
        let search = &doc.forms[0];
        assert_eq!(search.action, "https://example.com/results");
        assert_eq!(search.method, "get");
        assert_eq!(search.controls.len(), 4);
        assert!(search.controls[0].required);
        assert_eq!(search.controls[0].minlength, Some(2));
        assert!(search.controls[1].checked);
        assert_eq!(search.controls[2].value, "Popular");
        assert_eq!(search.controls[2].options.len(), 2);
        assert_eq!(doc.forms[1].controls[0].control_type, "password");
        assert_eq!(doc.forms[1].controls[0].value, "");
    }
}
