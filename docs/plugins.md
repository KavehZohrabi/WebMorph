# WebMorph plugins

WebMorph plugins are trusted Rust implementations linked into the application.
They receive a parsed `dom::Document`; they do not fetch pages, access terminal
state, run JavaScript, or load arbitrary code. All website retrieval remains
under the core HTTP client's URL, redirect, size, timeout, and content checks.

## Implementing a plugin

Implement `plugins::Plugin` and register it with `PluginRegistry::new()` (or add
it to the built-in registry). `matches` should be fast and should use the
normalized URL and document, not issue a network request. `priority` resolves
overlapping matches: the highest value runs; ties are resolved alphabetically
by plugin name. Only one plugin is applied to a document.

`process` can normalize the parsed document and return a `PluginContribution`
containing a website identity, existing semantic `transform::Component` values,
and optional navigation actions. Components are renderer-neutral; do not put
HTML or Ratatui widgets in them. Keep generated content bounded and preserve
existing interactive IDs when adapting content.

```rust
use reqwest::Url;
use webmorph::{dom::Document, plugins::{Plugin, PluginContribution, PluginError}};

struct ExamplePlugin;

impl Plugin for ExamplePlugin {
    fn name(&self) -> &'static str { "example" }

    fn matches(&self, url: &Url, _document: &Document) -> bool {
        url.host_str() == Some("example.org")
    }

    fn process(&self, document: &mut Document) -> Result<PluginContribution, PluginError> {
        // Adapt normalized content only; the core still owns networking.
        if document.title.is_empty() {
            document.title = "Example site".into();
        }
        Ok(PluginContribution {
            website_name: Some("Example".into()),
            ..PluginContribution::default()
        })
    }
}
```

Register a plugin with `registry.register(ExamplePlugin)`. Use
`registry.disable("example")`, `enable`, or `set_enabled` to control it for a
session. `PluginRegistry::default()` includes the built-in GitHub plugin;
`PluginRegistry::new()` starts empty for embedding and isolated tests.

## Failure and safety behavior

The registry catches a plugin panic or returned error, restores the original
document, logs a diagnostic, and lets the generic renderer continue. It also
rejects changed document URLs, unsafe non-HTTP(S) navigation/form targets,
invalid plugin navigation actions, and duplicate/empty interactive IDs in
plugin components. A rejected plugin contribution is discarded.

Plugins cannot add network permissions, bypass redirect/destination checks, or
read authentication data. The GitHub adapter recognizes public repository
overview, tree/file, README, and issues URLs on `github.com`; it uses the
already-fetched public document and adds repository shortcuts. It has no API
token, does not fetch private data, and does not implement GitHub-specific
authentication or JavaScript behavior.
