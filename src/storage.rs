use std::collections::HashMap;

use crate::dom::Document;

pub trait DocumentStore {
    fn get(&self, url: &str) -> Option<&Document>;
    fn put(&mut self, document: Document);
}

#[derive(Debug, Default)]
pub struct MemoryStore {
    documents: HashMap<String, Document>,
}

impl DocumentStore for MemoryStore {
    fn get(&self, url: &str) -> Option<&Document> {
        self.documents.get(url)
    }

    fn put(&mut self, document: Document) {
        self.documents.insert(document.url.clone(), document);
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentStore, MemoryStore};
    use crate::dom::Document;

    #[test]
    fn stores_documents_by_url() {
        let mut store = MemoryStore::default();
        store.put(Document::new("https://example.com", "Example"));
        assert_eq!(
            store
                .get("https://example.com")
                .map(|doc| doc.title.as_str()),
            Some("Example")
        );
    }
}
