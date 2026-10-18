# WebMorph

WebMorph is an open-source terminal application that turns fetched web pages
into a readable, keyboard-navigable terminal view.

## Getting started

```sh
cargo run -- https://example.com
```

Use `q` or `Esc` to exit, `j`/`k` or the arrow keys to scroll, and `g`/`G` to
jump to the top or bottom. Press `Ctrl+C` to shut down. Logging verbosity is
controlled with `RUST_LOG`, for example `RUST_LOG=debug cargo run -- URL`.

## Development

See [docs/architecture.md](docs/architecture.md) for module responsibilities
and extension points. Run `cargo fmt --check`, `cargo check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo test` before submitting
changes.
# WebMorph
