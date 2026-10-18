# Contributing to WebMorph

Thanks for considering a contribution. This project is in early development, so
small, focused changes with clear limitations are especially helpful.

## Before you start

- Search existing issues and pull requests before starting substantial work.
- For a large behavior or architecture change, open an issue first to discuss
  scope and compatibility.
- Do not include credentials, cookies, personal browsing history, private site
  content, or generated build output in commits or reports.
- Keep networking independent of the TUI, and keep HTML content untrusted.
- Do not add automatic scraping, script execution in lightweight mode, or
  network-policy bypasses.

## Development setup

Install the stable Rust toolchain with rustup and clone the repository. The
optional `browser-engine` feature additionally requires an installed compatible
Chromium executable only when running the browser integration path.

Run the same checks as CI before submitting:

```sh
cargo fmt --check
cargo check --locked
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

For Chromium feature changes, also build and test with
`--features browser-engine`; the rendered-page test may skip if a browser or
outbound network is unavailable.

## Pull requests

- Explain the user-visible change and relevant security implications.
- Add or update focused tests for behavior changes.
- Update README or docs when behavior, shortcuts, configuration, or limitations
  change.
- Keep public claims aligned with what the code and tests demonstrate.
- Do not include unrelated formatting or generated files.

## Code style

Use Rust's formatter, prefer clear errors at application boundaries, and avoid
logging form values or secrets. Tests should use local fixtures/servers and
should not depend on third-party websites unless explicitly conditional.
