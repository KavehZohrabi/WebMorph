//! Optional integration point for a future browser-backed renderer.

use crate::{dom::Document, error::Result};

pub trait BrowserEngine: Send + Sync {
    fn render(&self, url: &str) -> Result<Document>;
}
