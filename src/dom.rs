use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub metadata: PageMetadata,
    /// Hierarchical normalized content used by non-terminal renderers.
    #[serde(default)]
    pub content: Vec<ContentNode>,
    /// Stable, document-scoped controls and destinations.
    #[serde(default)]
    pub interactive: Vec<InteractiveElement>,
    #[serde(default)]
    pub forms: Vec<FormDefinition>,
    #[serde(default)]
    pub unsupported_elements: Vec<String>,
    /// Flat compatibility view consumed by the current terminal renderer.
    #[serde(default)]
    pub blocks: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageMetadata {
    pub base_url: Option<String>,
    pub language: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentNode {
    pub kind: ContentKind,
    #[serde(default)]
    pub children: Vec<ContentNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentKind {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph {
        text: String,
    },
    Link {
        text: String,
        href: Option<String>,
        interactive_id: Option<String>,
    },
    Image {
        src: Option<String>,
        alt: String,
        interactive_id: Option<String>,
    },
    List {
        ordered: bool,
    },
    ListItem {
        text: String,
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
    },
    Button {
        interactive_id: String,
    },
    Navigation {
        label: Option<String>,
    },
    Text {
        text: String,
    },
    Fallback {
        tag: String,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractiveElement {
    pub id: String,
    pub kind: InteractiveKind,
    pub label: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub input_type: Option<String>,
    pub href: Option<String>,
    pub form_action: Option<String>,
    pub placeholder: Option<String>,
    pub disabled: bool,
    #[serde(default)]
    pub html_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormDefinition {
    pub id: String,
    pub name: Option<String>,
    pub action: String,
    pub method: String,
    pub enctype: String,
    #[serde(default)]
    pub requires_javascript: bool,
    pub controls: Vec<FormControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormControl {
    pub interactive_id: String,
    pub name: Option<String>,
    pub control_type: String,
    pub value: String,
    pub label: String,
    pub required: bool,
    pub checked: bool,
    pub disabled: bool,
    pub options: Vec<(String, String)>,
    pub multiple: bool,
    pub minlength: Option<usize>,
    pub maxlength: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveKind {
    Link,
    Input,
    Button,
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
            metadata: PageMetadata::default(),
            content: Vec::new(),
            interactive: Vec::new(),
            forms: Vec::new(),
            unsupported_elements: Vec::new(),
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
