# Architecture

The crate is split by responsibility so the first implementation stays small
while later work can replace individual stages without changing the CLI.

## Request flow

`cli` parses arguments and initializes tracing. `app` coordinates the lifecycle:
it validates the input URL, asks `network` for the response body, passes that
body through `html` and `dom`, then gives the normalized document to
`transform`. `tui` presents the resulting page and owns terminal setup and
restoration. Ctrl+C is observed by the async application loop.

## Modules

- `cli`: command-line arguments and verbosity selection.
- `app`: application orchestration, shutdown, and error boundary.
- `network`: HTTP client and response handling.
- `html`: HTML document parsing with `scraper`.
- `dom`: normalized, serializable page and block data structures.
- `transform`: conversion from normalized page blocks to display lines.
- `tui`: terminal lifecycle, drawing, keyboard input, and scrolling.
- `navigation`: small in-memory page history model.
- `storage`: `DocumentStore` interface and in-memory implementation.
- `plugins`: minimal synchronous hooks for normalized documents.
- `browser`: optional browser-engine interface, enabled by the
  `browser-engine` Cargo feature; no browser runtime is bundled.
- `error`: shared error type used across application boundaries.

The normalized document is the shared boundary between parsing, plugins,
storage, and rendering. Networking and terminal I/O remain outside the parser
and transformation code, so those stages can be tested without a terminal or
network connection.

## Current scope

The HTML adapter extracts the title, headings, paragraphs, and links. It does
not execute JavaScript, load CSS, or model arbitrary interactive controls.
The browser feature currently defines an integration interface only.
