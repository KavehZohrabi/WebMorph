use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub url: String,
    pub title: String,
    pub blocks: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentBlock {
    Heading { level: u8, text: String },
    Paragraph { text: String },
    Link { text: String, href: String },
}

impl Document {
    pub fn new(url: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            title: title.into(),
            blocks: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentBlock, Document};

    #[test]
    fn constructs_empty_document() {
        let document = Document::new("https://example.com", "Example");
        assert_eq!(document.url, "https://example.com");
        assert!(document.blocks.is_empty());
    }

    #[test]
    fn blocks_are_comparable() {
        assert_eq!(
            ContentBlock::Heading {
                level: 1,
                text: "Title".into()
            },
            ContentBlock::Heading {
                level: 1,
                text: "Title".into()
            }
        );
    }
}
