# Website compatibility suite

WebMorph uses checked-in pages under `tests/fixtures/` for deterministic
compatibility coverage. The fixtures cover static HTML, documentation,
nested navigation and repeated article items, search forms, tables, and a
JavaScript application with dynamic updates and client-side routing.

Run the local suite and write a Markdown report:

```sh
cargo run --locked --bin compatibility-report -- --output target/compatibility-report.md
```

The report records the exact OS and architecture, Rust version, lightweight
client initialization and per-page loading times, parse and transform times,
rendered text size, interaction mapping, link navigation, GET/POST form
behavior, JavaScript-shell fallback detection, RSS deltas across two warmed
16-request batches where `/proc` is available, and loopback request totals. Each
fixture request has an 8 second timeout and the complete run has a 120 second
timeout. The runner exits nonzero when a deterministic compatibility assertion
fails.

Chromium checks are kept separate from normal platform builds. With a compatible
browser installed, run:

```sh
cargo run --locked --features browser-engine --bin compatibility-report -- \
  --output target/compatibility-report-chromium.md
```

The Chromium row is marked `SKIP` when the feature is absent or no executable is
installed. When available, it loads the local JavaScript app, activates a
dynamic button, follows client-side navigation, and checks that closing the tab
releases the engine's session slot. Browser navigation and interactions are
bounded by the configured timeout. RSS sampling measures the report process on
Linux; it does not measure Chromium child-process memory. Session-count checks
cover browser lifecycle cleanup.

Optional public-site checks are separate and never run as part of normal CI:

```sh
cargo run --locked --bin compatibility-report -- --live-smoke
```

They request `example.com` and `www.rust-lang.org` without changing user-agent
or access controls. DNS, connection, TLS, and timeout failures are reported as
network availability issues. Authentication, rate limits, access denials,
redirect policy outcomes, and unsupported content are reported separately from
implementation failures. Live responses are observational only and do not
change the deterministic test result.

GitHub Actions runs the deterministic suite and report on Linux, macOS, and
Windows. Reports are uploaded as workflow artifacts. A separate Ubuntu job
installs Chrome and runs the optional browser interaction suite. A workflow
artifact documents that CI run; it does not imply the same platforms were
tested locally.

## Limitations and next steps

- Local fixtures verify representative structures, not the full range of
  production site markup or anti-bot policies.
- The optional live smoke list is intentionally small and is not a release gate.
  An unreachable or restricted site is reported separately from a WebMorph
  failure.
- Process RSS is sampled only on Linux and excludes Chromium child processes.
  Chromium session-slot counts are checked after repeated close cycles; exact
  cross-platform memory accounting would need platform-specific process
  instrumentation.
- Chromium runtime interaction coverage currently runs on Ubuntu CI. Linux,
  macOS, and Windows matrix jobs run the deterministic lightweight suite.
