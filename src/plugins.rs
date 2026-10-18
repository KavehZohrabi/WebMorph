//! Trusted, in-process website adapters. Plugins only receive parsed public
//! documents; all fetching still goes through the core HTTP client.
use std::{
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
};

use reqwest::Url;
use thiserror::Error;

use crate::{
    dom::{ContentKind, Document, InteractiveElement, InteractiveKind},
    transform::{Component, ComponentKind, StatusState},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationAction {
    pub label: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginContribution {
    /// Friendly website identity to show alongside the page title.
    pub website_name: Option<String>,
    /// Additional renderer-neutral components; raw HTML is never accepted.
    pub components: Vec<Component>,
    /// Optional keyboard/action destinations supplied to the host UI.
    pub navigation: Vec<NavigationAction>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin rejected its document: {0}")]
    InvalidDocument(String),
    #[error("plugin produced invalid output: {0}")]
    InvalidOutput(String),
}

/// A plugin may recognize and annotate a normalized document and add semantic
/// components. It cannot fetch resources, access terminal state, or execute JS.
pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn priority(&self) -> i32 {
        0
    }
    fn enabled_by_default(&self) -> bool {
        true
    }
    fn matches(&self, url: &Url, document: &Document) -> bool;
    fn process(&self, _document: &mut Document) -> Result<PluginContribution, PluginError> {
        Ok(PluginContribution::default())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginReport {
    pub plugin_name: Option<String>,
    pub website_name: Option<String>,
    pub components: Vec<Component>,
    pub navigation: Vec<NavigationAction>,
    /// A diagnostic for logs/developer tooling; generic rendering remains usable.
    pub failure: Option<String>,
}

pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
    disabled: HashSet<String>,
    enabled: HashSet<String>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            disabled: HashSet::new(),
            enabled: HashSet::new(),
        }
    }

    pub fn register(&mut self, plugin: impl Plugin + 'static) {
        self.plugins.push(Box::new(plugin));
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        if enabled {
            self.disabled.remove(name);
            self.enabled.insert(name.to_owned());
        } else {
            self.enabled.remove(name);
            self.disabled.insert(name.to_owned());
        }
    }

    pub fn enable(&mut self, name: &str) {
        self.set_enabled(name, true);
    }
    pub fn disable(&mut self, name: &str) {
        self.set_enabled(name, false);
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        !self.disabled.contains(name)
            && self
                .plugins
                .iter()
                .find(|plugin| plugin.name() == name)
                .is_some_and(|plugin| self.enabled.contains(name) || plugin.enabled_by_default())
    }

    /// Processes one highest-priority match. Equal priorities are resolved by
    /// plugin name, making results independent of registration order.
    pub fn process(&self, document: &mut Document) -> PluginReport {
        let Ok(url) = Url::parse(&document.url) else {
            return PluginReport::default();
        };
        let mut matching: Vec<&dyn Plugin> = self
            .plugins
            .iter()
            .map(|plugin| plugin.as_ref())
            .filter(|plugin| self.is_enabled(plugin.name()))
            .filter(|plugin| {
                catch_unwind(AssertUnwindSafe(|| plugin.matches(&url, document))).unwrap_or(false)
            })
            .collect();
        matching.sort_by(|a, b| {
            b.priority()
                .cmp(&a.priority())
                .then_with(|| a.name().cmp(b.name()))
        });
        let Some(plugin) = matching.first() else {
            return PluginReport::default();
        };

        tracing::debug!(plugin = plugin.name(), "running website plugin");
        let original = document.clone();
        let result = catch_unwind(AssertUnwindSafe(|| plugin.process(document)));
        let contribution = match result {
            Ok(Ok(contribution)) => contribution,
            Ok(Err(error)) => {
                *document = original;
                return failed_report(plugin.name(), error.to_string());
            }
            Err(_) => {
                *document = original;
                return failed_report(plugin.name(), "plugin panicked".into());
            }
        };
        if let Err(error) = validate_plugin_output(&original, document, &contribution) {
            *document = original;
            return failed_report(plugin.name(), error.to_string());
        }
        PluginReport {
            plugin_name: Some(plugin.name().to_owned()),
            website_name: contribution.website_name,
            components: contribution.components,
            navigation: contribution.navigation,
            failure: None,
        }
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        let mut registry = Self {
            plugins: Vec::new(),
            disabled: HashSet::new(),
            enabled: HashSet::new(),
        };
        registry.register(GitHubPlugin);
        registry
    }
}

fn failed_report(name: &str, failure: String) -> PluginReport {
    tracing::warn!(plugin = name, %failure, "website plugin failed; using generic rendering");
    PluginReport {
        plugin_name: Some(name.to_owned()),
        failure: Some(failure),
        ..PluginReport::default()
    }
}

fn validate_plugin_output(
    original: &Document,
    changed: &Document,
    output: &PluginContribution,
) -> Result<(), PluginError> {
    if changed.url != original.url {
        return Err(PluginError::InvalidDocument(
            "plugins may not change the document URL".into(),
        ));
    }
    if !safe_http_url(&changed.url) {
        return Err(PluginError::InvalidDocument(
            "document URL must use HTTP or HTTPS".into(),
        ));
    }
    for link in &changed.interactive {
        if link.kind == InteractiveKind::Link
            && link.href.as_deref().is_some_and(|url| !safe_http_url(url))
        {
            return Err(PluginError::InvalidDocument(
                "unsafe navigation target".into(),
            ));
        }
        if link
            .form_action
            .as_deref()
            .is_some_and(|url| !safe_http_url(url))
        {
            return Err(PluginError::InvalidDocument("unsafe form action".into()));
        }
    }
    for form in &changed.forms {
        if !safe_http_url(&form.action) {
            return Err(PluginError::InvalidDocument("unsafe form action".into()));
        }
    }
    for node in &changed.content {
        validate_document_node(node)?;
    }
    for action in &output.navigation {
        if action.label.trim().is_empty() || !safe_http_url(&action.target) {
            return Err(PluginError::InvalidOutput(
                "invalid navigation action".into(),
            ));
        }
    }
    let mut ids = HashSet::new();
    for component in &output.components {
        validate_component(component, &mut ids)?;
    }
    Ok(())
}

fn validate_document_node(node: &crate::dom::ContentNode) -> Result<(), PluginError> {
    match &node.kind {
        ContentKind::Link {
            href: Some(href), ..
        } if !safe_http_url(href) => {
            return Err(PluginError::InvalidDocument("unsafe content link".into()));
        }
        ContentKind::Form {
            action: Some(action),
            ..
        } if !safe_http_url(action) => {
            return Err(PluginError::InvalidDocument(
                "unsafe content form action".into(),
            ));
        }
        _ => {}
    }
    for child in &node.children {
        validate_document_node(child)?;
    }
    Ok(())
}

fn validate_component(component: &Component, ids: &mut HashSet<String>) -> Result<(), PluginError> {
    match &component.kind {
        ComponentKind::Link {
            target: Some(target),
            ..
        } if !safe_http_url(target) => {
            return Err(PluginError::InvalidOutput(
                "unsafe component link target".into(),
            ));
        }
        ComponentKind::Link {
            interactive_id: Some(id),
            ..
        }
        | ComponentKind::Input {
            interactive_id: id, ..
        }
        | ComponentKind::Button {
            interactive_id: id, ..
        } if id.trim().is_empty() || !ids.insert(id.clone()) => {
            return Err(PluginError::InvalidOutput(
                "interactive identifiers must be non-empty and unique".into(),
            ));
        }
        _ => {}
    }
    for child in &component.children {
        validate_component(child, ids)?;
    }
    Ok(())
}

fn safe_http_url(value: &str) -> bool {
    Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

/// Adapts public GitHub repository pages with repository identity, shortcuts,
/// and a page-type indicator. It never contacts GitHub or requires a token.
pub struct GitHubPlugin;

impl Plugin for GitHubPlugin {
    fn name(&self) -> &'static str {
        "github"
    }
    fn priority(&self) -> i32 {
        100
    }

    fn matches(&self, url: &Url, _document: &Document) -> bool {
        url.host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
            && github_route(url).is_some()
    }

    fn process(&self, document: &mut Document) -> Result<PluginContribution, PluginError> {
        let url = Url::parse(&document.url)
            .map_err(|error| PluginError::InvalidDocument(error.to_string()))?;
        let Some(route) = github_route(&url) else {
            return Ok(PluginContribution::default());
        };
        let base = format!("https://github.com/{}/{}", route.owner, route.repository);
        let mut links = vec![
            ("github-code".to_owned(), "Code".to_owned(), base.clone()),
            (
                "github-issues".to_owned(),
                "Issues".to_owned(),
                format!("{base}/issues"),
            ),
        ];
        if route.kind != GitHubPageKind::Readme {
            let readme = if route.kind == GitHubPageKind::Repository {
                format!("{base}#readme")
            } else {
                format!("{base}/blob/HEAD/README.md")
            };
            links.push(("github-readme".to_owned(), "README".to_owned(), readme));
        }
        let shortcuts = Component {
            kind: ComponentKind::Navigation {
                label: Some("GitHub repository".into()),
            },
            children: links
                .into_iter()
                .map(|(id, text, target)| {
                    document.interactive.push(InteractiveElement {
                        id: id.clone(),
                        kind: InteractiveKind::Link,
                        label: text.clone(),
                        name: None,
                        value: None,
                        input_type: None,
                        href: Some(target.clone()),
                        form_action: None,
                        placeholder: None,
                        disabled: false,
                        html_id: None,
                    });
                    Component {
                        kind: ComponentKind::Link {
                            text,
                            target: Some(target),
                            interactive_id: Some(id),
                        },
                        children: vec![],
                    }
                })
                .collect(),
        };
        let kind_label = match route.kind {
            GitHubPageKind::Repository => "Repository overview",
            GitHubPageKind::Files => "Repository files",
            GitHubPageKind::Readme => "README",
            GitHubPageKind::Issues => "Issues",
        };
        let components = vec![
            shortcuts,
            Component {
                kind: ComponentKind::StatusIndicator {
                    label: format!("GitHub · {kind_label}"),
                    state: StatusState::Info,
                },
                children: vec![],
            },
        ];
        let navigation = components[0]
            .children
            .iter()
            .filter_map(|child| {
                if let ComponentKind::Link {
                    text,
                    target: Some(target),
                    ..
                } = &child.kind
                {
                    Some(NavigationAction {
                        label: text.clone(),
                        target: target.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();
        // The parser already exposes the normalized page title and content; the
        // plugin adds identity as metadata without replacing site content.
        Ok(PluginContribution {
            website_name: Some(format!("GitHub · {}/{}", route.owner, route.repository)),
            components,
            navigation,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitHubPageKind {
    Repository,
    Files,
    Readme,
    Issues,
}

struct GitHubRoute {
    owner: String,
    repository: String,
    kind: GitHubPageKind,
}

fn github_route(url: &Url) -> Option<GitHubRoute> {
    if !url.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segments: Vec<&str> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() < 2 {
        return None;
    }
    let owner = segments[0];
    let repository = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
    if owner.starts_with('@')
        || [
            "login",
            "signup",
            "settings",
            "notifications",
            "sessions",
            "password_reset",
        ]
        .contains(&owner)
    {
        return None;
    }
    if repository.is_empty() || repository.starts_with('.') {
        return None;
    }
    let remainder = &segments[2..];
    // Avoid recognizing private/admin and unrelated account routes as repo UI.
    let kind = match remainder {
        [] => GitHubPageKind::Repository,
        ["issues", ..] => GitHubPageKind::Issues,
        ["tree", ..] => GitHubPageKind::Files,
        ["blob", _, file, ..] if file.to_ascii_lowercase().starts_with("readme") => {
            GitHubPageKind::Readme
        }
        ["blob", ..] => GitHubPageKind::Files,
        ["", ..] => return None,
        _ => return None,
    };
    Some(GitHubRoute {
        owner: owner.to_owned(),
        repository: repository.to_owned(),
        kind,
    })
}

/// Keep unrecognized or invalid plugin output on the generic document path.
pub fn apply_report_to_page(page: &mut crate::transform::TuiPage, report: PluginReport) {
    if let Some(name) = report.website_name {
        page.title = format!("{} — {name}", page.title);
    }
    page.plugin_name = report.plugin_name;
    page.plugin_navigation = report.navigation;
    page.root.children.extend(report.components);
    if let Some(failure) = report.failure {
        tracing::debug!(%failure, "generic page fallback active");
    }
    page.lines = crate::transform::layout_components(&page.root, page.layout.terminal_width);
}

#[cfg(test)]
mod tests {
    use super::{
        GitHubPlugin, NavigationAction, Plugin, PluginContribution, PluginError, PluginRegistry,
    };
    use crate::{
        dom::{ContentKind, ContentNode, Document},
        transform::ComponentKind,
    };
    use reqwest::Url;

    struct SetTitle;
    impl Plugin for SetTitle {
        fn name(&self) -> &'static str {
            "title"
        }
        fn matches(&self, url: &Url, _: &Document) -> bool {
            url.host_str() == Some("example.com")
        }
        fn process(&self, document: &mut Document) -> Result<PluginContribution, PluginError> {
            document.title = "Updated".into();
            Ok(PluginContribution::default())
        }
    }

    struct Priority(&'static str, i32);
    impl Plugin for Priority {
        fn name(&self) -> &'static str {
            self.0
        }
        fn priority(&self) -> i32 {
            self.1
        }
        fn matches(&self, _: &Url, _: &Document) -> bool {
            true
        }
        fn process(&self, _: &mut Document) -> Result<PluginContribution, PluginError> {
            Ok(PluginContribution {
                website_name: Some(self.0.into()),
                ..PluginContribution::default()
            })
        }
    }

    struct InvalidOutput;
    impl Plugin for InvalidOutput {
        fn name(&self) -> &'static str {
            "invalid"
        }
        fn matches(&self, _: &Url, _: &Document) -> bool {
            true
        }
        fn process(&self, _: &mut Document) -> Result<PluginContribution, PluginError> {
            Ok(PluginContribution {
                navigation: vec![NavigationAction {
                    label: "bad".into(),
                    target: "javascript:alert(1)".into(),
                }],
                ..PluginContribution::default()
            })
        }
    }

    #[test]
    fn registry_selects_plugin_by_match_and_changes_document() {
        let mut registry = PluginRegistry::new();
        registry.register(SetTitle);
        let mut document = Document::new("https://example.com", "Original");
        let report = registry.process(&mut document);
        assert_eq!(document.title, "Updated");
        assert_eq!(report.plugin_name.as_deref(), Some("title"));
    }

    #[test]
    fn priority_and_name_tie_breaking_are_deterministic() {
        let mut registry = PluginRegistry::new();
        registry.register(Priority("zeta", 5));
        registry.register(Priority("alpha", 5));
        registry.register(Priority("high", 6));
        let mut document = Document::new("https://example.com", "Original");
        assert_eq!(
            registry.process(&mut document).plugin_name.as_deref(),
            Some("high")
        );
        let mut tied = PluginRegistry::new();
        tied.register(Priority("zeta", 5));
        tied.register(Priority("alpha", 5));
        assert_eq!(
            tied.process(&mut document).plugin_name.as_deref(),
            Some("alpha")
        );
    }

    #[test]
    fn supports_enable_disable_and_generic_fallback() {
        let mut registry = PluginRegistry::new();
        registry.register(SetTitle);
        registry.disable("title");
        let mut document = Document::new("https://example.com", "Original");
        assert!(registry.process(&mut document).plugin_name.is_none());
        assert_eq!(document.title, "Original");
        registry.enable("title");
        assert_eq!(
            registry.process(&mut document).plugin_name.as_deref(),
            Some("title")
        );
    }

    #[test]
    fn invalid_plugin_output_is_discarded_and_document_restored() {
        let mut registry = PluginRegistry::new();
        registry.register(InvalidOutput);
        let mut document = Document::new("https://example.com", "Original");
        document.content.push(ContentNode {
            kind: ContentKind::Paragraph {
                text: "safe generic".into(),
            },
            children: vec![],
        });
        let report = registry.process(&mut document);
        assert!(report.failure.is_some());
        assert!(report.components.is_empty());
        assert_eq!(document.title, "Original");
        assert_eq!(document.content.len(), 1);
    }

    #[test]
    fn github_plugin_recognizes_public_repository_pages_and_generates_shortcuts() {
        let plugin = GitHubPlugin;
        let samples = [
            ("https://github.com/rust-lang/rust", true),
            (
                "https://github.com/rust-lang/rust/tree/master/library",
                true,
            ),
            (
                "https://github.com/rust-lang/rust/blob/master/README.md",
                true,
            ),
            ("https://github.com/rust-lang/rust/issues", true),
            ("https://github.com/login", false),
            ("https://github.com/rust-lang", false),
        ];
        for (url, expected) in samples {
            let parsed = Url::parse(url).unwrap();
            assert_eq!(
                plugin.matches(&parsed, &Document::new(url, "Page")),
                expected,
                "{url}"
            );
        }
        let mut document = Document::new("https://github.com/rust-lang/rust/issues", "Issues");
        let report = PluginRegistry::default().process(&mut document);
        assert_eq!(
            report.website_name.as_deref(),
            Some("GitHub · rust-lang/rust")
        );
        assert_eq!(report.navigation.len(), 3);
        assert_eq!(
            document
                .interactive
                .iter()
                .filter(|element| element.id.starts_with("github-"))
                .count(),
            3
        );
        assert!(report.components.iter().any(|component| matches!(component.kind, ComponentKind::StatusIndicator { ref label, .. } if label.contains("Issues"))));
        let mut page = crate::transform::transform(&document);
        super::apply_report_to_page(&mut page, report);
        assert_eq!(page.plugin_name.as_deref(), Some("github"));
        assert_eq!(page.plugin_navigation.len(), 3);
        assert!(page.root.children.iter().any(|component| matches!(component.kind, ComponentKind::Navigation { ref label } if label.as_deref() == Some("GitHub repository"))));
        assert!(page.root.children.iter().flat_map(|component| &component.children).any(|component| matches!(component.kind, ComponentKind::Link { interactive_id: Some(ref id), .. } if id == "github-issues")));
    }
}
