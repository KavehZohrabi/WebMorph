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

## Rendering and interaction

The backend uses `chromiumoxide` to launch sandboxed headless Chromium over the
Chrome DevTools Protocol. Chromium stays alive per WebMorph tab, so actions keep
the page's JavaScript heap and in-memory session state. WebMorph extracts the
rendered DOM after navigation and after actions, then feeds it through the
existing normalizer and TUI transformation pipeline. Interactive DOM elements
receive stable keys for that snapshot; WebMorph maps those keys back to browser
elements for real clicks, text entry, select changes, form submission, and
scrolling. The TUI refreshes the normalized page after each action. Same-origin
links are clicked in the page; links to another origin replace that tab's
browser session and navigate there.

When explicitly enabled as a fallback, WebMorph uses Chromium only after the
lightweight engine detects a likely JavaScript application shell. That tab then
stays on its Chromium session for subsequent navigation and interactions. The
lightweight engine remains the default, and fallback is only used when the user
enables it.

## Safety and resource bounds

Chromium runs with its normal sandbox enabled; WebMorph does not add
`--no-sandbox` or ignore HTTPS certificate errors. DevTools uses a random
ephemeral port bound to `127.0.0.1` and is available only while the owned
browser process is active. Each WebMorph tab gets its own temporary incognito
profile, removed when that tab closes or the application exits. The backend
caps active sessions, renderer processes, and V8 heap size. Cancellation
interrupts active work; normal close requests browser shutdown and waits for
process exit.

By default, WebMorph validates the requested destination and routes Chromium
HTTP/HTTPS traffic through a per-session loopback proxy. The proxy validates
each destination, resolves it, rejects loopback, private, link-local,
multicast, and other non-public addresses, then connects to the validated IP.
Chromium direct DNS is blocked. This allows public scripts, images, and API
hosts while keeping page content from directly reaching local services. Every
top-level navigation is validated, and a cross-host navigation gets a fresh
per-tab browser process. An opt-in
`allow_private_network` setting exists for local integration testing and
embeddings; it should not be enabled for untrusted websites.

Navigation has a 30-second default timeout; rendered DOM output is capped at
8 MiB; at most four tab sessions may be active; each browser process is capped
at four renderer processes and a 512 MiB V8 heap (defaults are two renderers
and 256 MiB). The per-session proxy accepts at most 32 simultaneous
connections. These browser flags and limits bound common resource use, but they
are not operating-system CPU or total-memory quotas.

## Supported actions and limitations

The TUI maps links, buttons, text/search/password inputs, textareas, checkboxes,
radio buttons, selects, and submit buttons to browser DOM actions. It captures
visible text and basic accessible names from labels, `aria-label`, and `title`;
it does not implement the full Chromium accessibility tree. It does not support
file uploads, downloads, WebSockets, browser permission prompts, native dialogs,
drag and drop, arbitrary keyboard shortcuts inside a page, or multi-select controls.
Downloads are denied. Cross-origin iframe controls may not be mapped. CAPTCHA,
anti-bot gates, and authentication restrictions are not bypassed. Site-specific
behavior can still depend on browser features or network destinations WebMorph
intentionally blocks.

The conditional integration tests use local pages and run only when a
compatible Chromium executable is installed and can be launched.
