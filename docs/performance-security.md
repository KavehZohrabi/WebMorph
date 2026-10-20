# Performance, security, and reliability notes

## Measurements

`benches/pipeline.rs` is a dependency-free, optimized benchmark for HTML parsing,
document transformation, and terminal layout. It uses the same generated fixture
before and after the changes: 504,081 bytes, 2,000 paragraphs, 2,000 links, and
1,000 labeled forms. Results are mean wall time over 30 iterations on the same
host. The latest run also exercised interactive and reader viewport paths. This
is a targeted microbenchmark, not an allocation profile or full terminal-emulator
benchmark; small differences should be treated as noise.

| Stage | Earlier pipeline run | TASK-0016 run |
| --- | ---: | ---: |
| HTML parse | 25.46 ms | 38.42 ms |
| Document transform | 8.93 ms | 13.36 ms |
| Full terminal layout | 5.49 ms | 7.91 ms |
| Interactive viewport window | — | 6.72 ms |
| Reader viewport window | — | 6.47 ms |
| Cached 20-row copy from 4,096-line window | — | 0.0004 ms |

The latest run is slower than the earlier pipeline run on all three shared
stages. It includes additional semantic-region and interactive-component work;
the benchmark does not isolate the cost of those additions, so the timings do
not establish a specific cause. The viewport path retains bounded lines but
traverses the component tree when rebuilding a window. Reusing the cached window
and copying 20 visible rows is much cheaper than rebuilding it. These timings do
not include Ratatui or terminal-device painting.

The earlier baseline comparison is recorded in the repository history. Results
measure CPU processing only; they do not include network latency, SQLite I/O, or
terminal-device refresh time.

## Changes

- Form extraction builds temporary indexes for interactive controls and labels,
  replacing repeated full-document searches with keyed lookups.
- Component transformation indexes interactive metadata once, so mapping each
  form control does not rescan the entire interactive-element list.
- The UI releases each loaded page's compatibility line snapshot and renders
  from its semantic tree into a cached 4,096-line scrolling window. Scroll and
  mouse row mapping use document-line coordinates. A window is rebuilt when the
  viewport moves outside its cached range.
- Cached lines are capped at 8 MiB; inactive tab windows are discarded. The
  normalized document tree remains in each open tab.
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
- The 8 MiB cap applies to active-tab rendered lines, not normalized trees held
  by open tabs or the downloaded body. Those remain bounded by the response
  limit and HTML size; there is no total memory quota across tabs.
- The benchmark does not profile allocator counts, peak resident memory, SQLite
  latency, or actual terminal repaint time. Those need dedicated profiling on
  representative machines before further optimization.
