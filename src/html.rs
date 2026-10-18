use scraper::{Html, Selector};

use crate::dom::{ContentBlock, Document};

pub fn parse(url: &str, source: &str) -> Document {
    let html = Html::parse_document(source);
    let title_selector = Selector::parse("title").expect("static selector");
    let title = html
        .select(&title_selector)
        .next()
        .map(text_of)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| url.to_owned());
    let mut document = Document::new(url, title);

    let content_selector =
        Selector::parse("h1, h2, h3, h4, h5, h6, p, a[href]").expect("static selector");
    for element in html.select(&content_selector) {
        let text = text_of(element);
        if text.is_empty() {
            continue;
        }
        match element.value().name() {
            "h1" => document
                .blocks
                .push(ContentBlock::Heading { level: 1, text }),
            "h2" => document
                .blocks
                .push(ContentBlock::Heading { level: 2, text }),
            "h3" => document
                .blocks
                .push(ContentBlock::Heading { level: 3, text }),
            "h4" => document
                .blocks
                .push(ContentBlock::Heading { level: 4, text }),
            "h5" => document
                .blocks
                .push(ContentBlock::Heading { level: 5, text }),
            "h6" => document
                .blocks
                .push(ContentBlock::Heading { level: 6, text }),
            "p" => document.blocks.push(ContentBlock::Paragraph { text }),
            "a" => {
                let Some(href) = element.value().attr("href") else {
                    continue;
                };
                document.blocks.push(ContentBlock::Link {
                    text,
                    href: href.to_owned(),
                });
            }
            _ => {}
        }
    }
    document
}

fn text_of(element: scraper::ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use crate::dom::ContentBlock;

    use super::parse;

    #[test]
    fn extracts_page_content() {
        let document = parse(
            "https://example.com",
            "<title>Demo</title><h1>Hello</h1><p>Readable text</p><a href='/next'>Next</a>",
        );
        assert_eq!(document.title, "Demo");
        assert_eq!(document.blocks.len(), 3);
        assert!(
            matches!(&document.blocks[0], ContentBlock::Heading { level: 1, text } if text == "Hello")
        );
        assert!(matches!(&document.blocks[2], ContentBlock::Link { href, .. } if href == "/next"));
    }
}
