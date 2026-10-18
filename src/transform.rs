use crate::dom::{ContentBlock, Document};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiPage {
    pub title: String,
    pub lines: Vec<String>,
}

pub fn transform(document: &Document) -> TuiPage {
    let lines = document
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Heading { level, text } => {
                format!("{} {text}", "#".repeat(*level as usize))
            }
            ContentBlock::Paragraph { text } => text.clone(),
            ContentBlock::Link { text, href } => format!("{text} ({href})"),
        })
        .collect();
    TuiPage {
        title: document.title.clone(),
        lines,
    }
}

#[cfg(test)]
mod tests {
    use crate::dom::{ContentBlock, Document};

    use super::transform;

    #[test]
    fn transforms_blocks_into_readable_lines() {
        let mut document = Document::new("https://example.com", "Page");
        document.blocks = vec![
            ContentBlock::Heading {
                level: 2,
                text: "Heading".into(),
            },
            ContentBlock::Link {
                text: "Home".into(),
                href: "/".into(),
            },
        ];
        let page = transform(&document);
        assert_eq!(
            page.lines,
            vec!["## Heading".to_owned(), "Home (/)".to_owned()]
        );
    }
}
