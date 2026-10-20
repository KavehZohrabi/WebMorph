# WebMorph

WebMorph is an early-stage, open-source terminal web reader. It fetches public
HTTP(S) pages and turns their readable content into a keyboard- and
mouse-navigable terminal interface. It is designed for reading and lightweight
interaction; it is not a full browser.

## Features

- Lightweight HTTP/HTTPS engine is the default, with request timeouts, response
  limits, compressed-response support, redirect checks, and cancellation.
- Terminal interface with multiple tabs, independent navigation history,
  scrolling, address entry, reader/interactive/compact presentation modes, and
  explicit external-browser opening.
- HTML extraction for headings, paragraphs, links, lists, tables, images,
  navigation landmarks, main/sidebar regions, repeated article items, open
  dialogs, disclosures, and common form controls.
- Basic GET and URL-encoded POST forms, with practical HTML validation and a
  confirmation before submitting forms. GET forms containing password fields
  are refused so passwords cannot be exposed in URLs or browsing history.
- Public GitHub page recognition through a trusted built-in plugin.
- Local SQLite storage APIs for visit history, bookmarks, and settings; private
  sessions skip history persistence.
- Optional Chromium-backed rendering, selected explicitly at launch.
- Website text is sanitized before terminal output. Scripts are not executed by
  the lightweight engine.

## Installation

The first release is built from source with a current stable Rust toolchain
(edition 2024). Clone the repository, then install the executable:

```sh
git clone https://github.com/kaveh-zohrabi/WebMorph.git
cd WebMorph
cargo install --path .
```

To install from a local checkout without adding it to the Cargo bin directory,
use `cargo build --release`; the executable is written to `target/release/`.

## Build

```sh
cargo build --release
```

To include the optional Chromium backend at compile time:

```sh
cargo build --release --features browser-engine
```

Chromium is not bundled or downloaded. See
[docs/browser-engine.md](docs/browser-engine.md) for runtime requirements and
limitations.

## Usage

```sh
webmorph https://example.com
webmorph                         # opens the configured default homepage
webmorph --private https://example.com
webmorph --engine chromium https://example.com
webmorph --help
```

WebMorph only opens HTTP and HTTPS URLs. Local, private, and other restricted
network destinations are blocked by the default security policy. Links open in
the current tab with Enter; use Shift+Enter to open a link in a new tab. Use
Alt+Enter to ask the operating system to open the selected URL in its external
browser.

The CLI's current options are:

```text
Transform a website into an interactive terminal view

Usage: webmorph [OPTIONS] [URL]

Arguments:
  [URL]  Website URL to open

Options:
      --private                Do not read or write persistent browsing history for this session
      --engine <ENGINE>        Select the rendering engine (Chromium requires `--features browser-engine`) [default: lightweight] [possible values: lightweight, chromium]
  -v, --verbosity <VERBOSITY>  Set log verbosity (overridden by `RUST_LOG` when present) [default: warn]
  -h, --help                   Print help
  -V, --version                Print version
```

## Keyboard and mouse

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Move focus among controls |
| `Enter` | Edit or activate the focused control/link |
| `Space` | Toggle a checkbox or radio option |
| `Shift+Enter` | Open the focused link in a new tab |
| `Alt+Enter` | Open the focused link in the system browser |
| Arrow keys or `j` / `k` | Scroll page content; arrows also switch tabs while the tab bar is focused and change a select option while editing it |
| `g` / `Home`, `G` / `End` | With page focus, jump to the start or end of page |
| `Ctrl+L` | Focus the address bar |
| `Ctrl+T`, `Ctrl+W` | Open or close a tab |
| `Ctrl+Tab`, `Ctrl+Shift+Tab` | Next or previous tab |
| `h`, `[` / `]`, `r` | With page focus: site root, back/forward, reload |
| `t` | With page focus, cycle automatic, dark, and light themes for the current session |
| `v` | With page focus, cycle Interactive, Reader, and Compact presentation modes |
| `?` or `F1` | Show help |
| `q` (outside text entry), `Ctrl+Q` | Quit |
| `Esc` | Dismiss a dialog, cancel a pending request, or leave the address bar/field editor |

Mouse clicks select visible controls and links. The wheel scrolls the page.
Terminal key handling varies by emulator; keyboard alternatives remain
available for core interactions.

Interactive mode shows the page structure and available controls. Reader mode
hides navigation and sidebars and opens disclosure sections for reading.
Compact mode reduces spacing and stacks tables for narrow layouts. Click the
mode label in the status row to cycle views with a mouse.

## Configuration and privacy

WebMorph currently has no settings editor or configuration file. Its storage
layer supports a default homepage, theme preference, timeout, User-Agent, mouse
preference, and engine preference, but only the timeout, User-Agent, and
homepage are currently consumed by application startup. Theme selection is
session-only. Browsing history is written to an OS-specific local application
data directory. Bookmarks and history-management methods exist in the storage
API but are not yet exposed as TUI commands.

Use `--private` to open a session that does not read or write persistent visit
history. It does not isolate or clear pre-existing browser data; WebMorph does
not store cookies or passwords. Form password fields are held transiently in
memory for submission and cleared when the tab/document state is discarded.

Logging defaults to warnings. Set `RUST_LOG=webmorph=debug` for diagnostics;
avoid sharing logs that may contain URLs.

## Plugin architecture

Built-in Rust plugins receive the already parsed normalized document. They do
not fetch pages or access the terminal. The registry resolves matches by
priority and falls back to the generic renderer. The built-in GitHub plugin
recognizes public repository overview, file/tree, README, and issue-listing
pages. Arbitrary dynamic plugins and authenticated/private GitHub support are
not implemented. See [docs/plugins.md](docs/plugins.md).

## Architecture

The application separates networking, HTML parsing and document normalization,
plugin processing, terminal-oriented transformation, SQLite storage, optional
browser loading, and the Ratatui/Crossterm UI. See
[docs/architecture.md](docs/architecture.md) for module boundaries.

Linux, macOS, and Windows builds are targeted by CI. Only Linux has been
executed in the current release-preparation environment; the other platforms
are not claimed as locally tested. Crossterm and the OS-specific external-open
commands are platform dependent, so CI results and reports from real terminals
remain important.

## Known limitations

- Lightweight mode does not run JavaScript, execute scripts, apply CSS, or
  reproduce browser layout. Many modern application sites therefore show only
  their server-rendered HTML or a limited page.
- Forms driven by JavaScript, CAPTCHA, file upload, multipart encoding, and
  complex browser authentication flows are unsupported. WebMorph does not
  bypass authentication or CAPTCHA.
- Chromium is optional and keeps an isolated session per tab for JavaScript-backed interactions. It supports mapped clicks, text entry, selects, form submission, and scrolling, with a restrictive network/resource policy. It is not a complete browser and does not persist cookies or profiles.
- Bookmarks, history search/deletion, and settings are storage APIs and are not
  yet exposed as interactive application screens.
- Compatibility depends on the terminal's Unicode, mouse, and key-event
  support. No claim is made that all sites or terminals work identically.
- Large documents are rendered from a bounded scrolling window. Layout still
  walks the component tree when that window moves outside the cached range, so
  exceptionally large, deeply nested pages can take longer to scroll.
- Lightweight mode cannot expand a `<details>` section because it does not run
  page scripts; such controls are labeled as requiring Chromium. Interactive
  disclosure and dialog actions are available only when Chromium supplies a
  browser-backed element mapping.

## Contributing

Bug reports and focused contributions are welcome. Read
[CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request. Run formatting,
checks, linting, and tests locally. Please do not include credentials, cookies,
private browsing data, or unlicensed third-party content in issues or patches.

## Security

Please report security issues privately as described in [SECURITY.md](SECURITY.md).

## License

WebMorph is distributed under the MIT License; see [LICENSE](LICENSE). See
[docs/dependency-licenses.md](docs/dependency-licenses.md) for the review of
third-party crate license metadata.
