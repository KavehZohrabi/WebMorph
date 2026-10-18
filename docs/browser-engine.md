# Optional Chromium engine

WebMorph's `lightweight` HTTP engine remains the default. To include the
optional Chromium backend, build with:

```sh
cargo build --features browser-engine
```

Select it explicitly at launch:

```sh
webmorph --engine chromium https://example.com
```

WebMorph detects an existing Chrome, Chromium, or Edge executable through the
standard executable search and `CHROME`. It never downloads or installs one.
Without the feature, selecting Chromium produces an error explaining how to
enable it. A missing executable also produces a direct error.

The backend uses the maintained `chromiumoxide` crate to launch a fresh,
sandboxed headless browser over the Chrome DevTools Protocol. It waits for page
navigation and `document.readyState == "complete"`, executes page JavaScript,
captures the live document HTML and final URL, then feeds that DOM through the
existing normalizer and TUI transformation pipeline. Links and common form
controls receive the existing WebMorph interactive identifiers and keyboard
handling. `ChromiumEngine::execute_javascript` is available to embeddings that
need to evaluate an explicit JavaScript expression.

## Safety and resource bounds

Chromium runs with its normal sandbox enabled; WebMorph does not add
`--no-sandbox` or ignore HTTPS certificate errors. DevTools uses a random
ephemeral port bound to `127.0.0.1` and is available only while the short-lived
browser process is active. Each page gets an isolated temporary incognito
profile which is removed after the browser closes. Cancellation drops the
backend task and Chromiumoxide terminates its child process; normal completion
requests an orderly close and waits for process exit.

Before launch, WebMorph resolves the requested host and rejects loopback,
private, link-local, multicast, and other non-public addresses. Chromium's
resolver is pinned to the validated address for that host, with other host
lookups blocked. This prevents third-party resources and cross-host redirects
from loading in Chromium. It is intentionally restrictive, and some pages may
lose images, scripts, or API-backed content as a result.

Navigation has a 30-second default timeout; rendered DOM output is capped at
8 MiB; Chromium is limited to two renderer processes and a 256 MiB V8 heap.
These browser flags bound common sources of resource use, but they are not
operating-system CPU or total-memory quotas.

The backend is not a complete interactive browser. It does not keep Chromium
alive across navigations, preserve browser cookies, support arbitrary
JavaScript-driven form submissions through the TUI, or guarantee that every
site works. The optional integration test runs only when Chromium is installed
and the environment can launch it and access the test page.
