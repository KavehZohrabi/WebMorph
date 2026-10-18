# Architecture

WebMorph separates fetching, parsing, document normalization, terminal layout,
persistence, and terminal interaction. This keeps the lightweight request and
rendering pipeline independent from the optional Chromium loader.

## Request and render flow

`cli` parses launch options and initializes tracing. `app` loads persisted
settings, selects the engine, and creates the TUI. The TUI owns tabs, address
entry, input events, form interaction, and navigation tasks. In lightweight
mode, `network` validates and fetches the URL; `html` parses the response into
`dom::Document`; `plugins` optionally adapts it; `transform` creates terminal
components and visible lines; `tui` renders those lines using Ratatui and
Crossterm. In Chromium mode, `browser` launches a feature-gated browser process,
extracts the rendered page HTML, and returns it to the same parser and renderer.

## Modules

- `cli`: command-line arguments (`URL`, `--private`, `--engine`, verbosity).
- `app`: startup configuration and lifetime coordination.
- `network`: asynchronous HTTP client, URL and destination policy, cancellation,
  redirects, content checks, request bounds, and form submission.
- `html`: HTML parsing and extraction of readable content and common controls.
- `dom`: normalized page, metadata, content, forms, and interactive elements.
- `transform`: renderer-independent semantic components and terminal layout.
- `tui`: terminal lifecycle, tabs, key/mouse handling, forms, and rendering.
- `navigation`: small in-memory URL history helper.
- `storage`: SQLite history, bookmarks, settings APIs, migrations, plus a
  separate in-memory document store abstraction.
- `plugins`: trusted in-process plugin interface, registry, and built-in GitHub
  adapter.
- `browser`: lightweight backend trait and optional Chromium implementation.
- `error`: application-level errors used at module boundaries.

The normalized document is the boundary between parsing, plugins, and layout.
The persistent storage APIs are separate from the UI; several management
operations are not currently exposed as TUI screens. Browsing-history recording
is integrated with page navigation unless private mode is active.

## Current scope

The lightweight engine does not execute JavaScript or apply CSS. It extracts
readable HTML and common form controls. Chromium mode is optional and separately
selected; it executes page scripts inside Chromium and normalizes the resulting
HTML, but it is not a persistent full browser. See
[docs/browser-engine.md](browser-engine.md) and [docs/plugins.md](plugins.md).
