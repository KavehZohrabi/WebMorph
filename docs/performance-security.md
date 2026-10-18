# Performance, security, and reliability notes

## Measurements

`benches/pipeline.rs` is a dependency-free, optimized benchmark for HTML parsing,
document transformation, and terminal layout. It uses the same generated fixture
before and after the changes: 504,081 bytes, 2,000 paragraphs, 2,000 links, and
1,000 labeled forms. Results are mean wall time over 30 iterations on the same
host. This is a targeted microbenchmark, not an allocation profile or a full
terminal-emulator benchmark; small differences should be treated as noise.

| Stage | Committed baseline | Updated pipeline |
| --- | ---: | ---: |
| HTML parse | 347.71 ms | 25.46 ms |
| Document transform | 20.51 ms | 8.93 ms |
| Terminal layout | 5.72 ms | 5.49 ms |

On this form-heavy fixture, parsing was 13.7× faster (92.7% less time),
transformation was 2.3× faster (56.5% less time), and the standalone layout
pass was 4.0% faster. A 20-row visible-window copy measured 0.0004 ms; this
isolates the slice/copy work and is not a full Ratatui render measurement.

The baseline revision is `55975f9`. Results measure CPU processing only; they do
not include network latency, SQLite I/O, or terminal device refresh time.

## Changes

- Form extraction builds temporary indexes for interactive controls and labels,
  replacing repeated full-document searches with keyed lookups.
- Component transformation indexes interactive metadata once, so mapping each
  form control does not rescan the entire interactive-element list.
- The terminal renders only the visible line window when scrolling, while keeping
  scroll and mouse row mapping in document-line coordinates.
- A one-page terminal-layout cache is kept for the active tab and current width;
  it is capped at 8 MiB and discarded on navigation, resize, or tab switch.
- Terminal control characters are removed from rendered web content, form
  values, titles, errors, and addresses. The sanitizer edits owned lines in
  place where possible.
- Request concurrency permits now cover DNS policy checks as well as HTTP work.
  Cancellation interrupts DNS lookup, connection, and response streaming.

## Security and reliability review

- Navigation accepts only HTTP and HTTPS URLs without embedded credentials.
- Private, loopback, link-local, carrier-grade NAT, documentation, benchmarking,
  reserved, and non-global IPv6 ranges are rejected by default. Reqwest's own
  resolver repeats address validation at connection time to reduce DNS-rebinding
  exposure.
- Redirects remain same-host, obey the configured redirect limit, reject blocked
  destinations, and cannot downgrade HTTPS to HTTP.
- Response bytes remain bounded by the configured decompressed-body size limit;
  downloaded scripts are treated as inert HTML text by the lightweight engine.
- Website text cannot send terminal escape/control characters to the terminal.
- Local HTTP tests cover mid-response cancellation and the configured parallel
  request cap. Parser tests cover malformed, empty, and 3,000-paragraph documents;
  UI tests cover repeated tab selection and visible-window scrolling.

## Remaining limitations

- DNS is checked before the request and again by reqwest's resolver, which may
  perform two lookups for a hostname. This favors destination safety over lookup
  latency; no DNS cache was added.
- The 8 MiB layout cache applies to rendered lines, not the normalized document
  tree or downloaded body. Those remain bounded by the network response limit
  and the HTML size itself.
- The benchmark does not profile allocator counts, peak resident memory, SQLite
  latency, or actual terminal repaint time. Those need dedicated profiling on
  representative machines before further optimization.
