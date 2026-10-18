use crate::dom::Document;

pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;

    fn process(&self, _document: &mut Document) {}
}

#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginRegistry {
    pub fn register(&mut self, plugin: impl Plugin + 'static) {
        self.plugins.push(Box::new(plugin));
    }

    pub fn process(&self, document: &mut Document) {
        for plugin in &self.plugins {
            tracing::debug!(plugin = plugin.name(), "running plugin");
            plugin.process(document);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Plugin, PluginRegistry};
    use crate::dom::Document;

    struct SetTitle;

    impl Plugin for SetTitle {
        fn name(&self) -> &'static str {
            "set-title"
        }

        fn process(&self, document: &mut Document) {
            document.title = "Updated".into();
        }
    }

    #[test]
    fn registry_runs_registered_plugins() {
        let mut registry = PluginRegistry::default();
        registry.register(SetTitle);
        let mut document = Document::new("https://example.com", "Original");
        registry.process(&mut document);
        assert_eq!(document.title, "Updated");
    }
}
