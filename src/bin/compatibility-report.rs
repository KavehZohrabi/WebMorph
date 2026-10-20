use std::{
    env, fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use webmorph::{
    compatibility::{PageAssessment, assess},
    error::WebMorphError,
    html,
    network::{HttpClient, HttpConfig},
    transform::transform,
};

const STATIC: &str = include_str!("../../tests/fixtures/static.html");
const DOCUMENTATION: &str = include_str!("../../tests/fixtures/documentation.html");
const NAVIGATION: &str = include_str!("../../tests/fixtures/complex-navigation.html");
const SEARCH: &str = include_str!("../../tests/fixtures/search-form.html");
const TABLES: &str = include_str!("../../tests/fixtures/tables.html");
const JAVASCRIPT_APP: &str = include_str!("../../tests/fixtures/javascript-app.html");

struct FixtureServer {
    origin: String,
    requests: Arc<AtomicUsize>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl FixtureServer {
    async fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("could not start local fixture server: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("could not inspect fixture server: {error}"))?;
        let requests = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&requests);
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = task_stop.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        request_count.fetch_add(1, Ordering::Relaxed);
                        tokio::spawn(async move { serve_fixture(stream).await; });
                    }
                }
            }
        });
        Ok(Self {
            origin: format!("http://{address}"),
            requests,
            stop,
            task,
        })
    }

    async fn close(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&bytes);
        let Some(header_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let content_length = text[..header_end]
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if bytes.len() >= header_end + 4 + content_length {
            break;
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

async fn serve_fixture(mut stream: TcpStream) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    let mut request_lines = request.lines();
    let mut request_line = request_lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or("GET");
    let target = request_line.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or(target);
    let post_body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
    let body = match (method, path) {
        (_, "/static") => STATIC,
        (_, "/documentation") => DOCUMENTATION,
        (_, "/navigation") => NAVIGATION,
        (_, "/search") => SEARCH,
        (_, "/tables") => TABLES,
        (_, "/javascript-app") => JAVASCRIPT_APP,
        (_, "/documentation/next") => {
            "<main><h1>Next chapter</h1><p>Link navigation reached this page.</p></main>"
        }
        ("GET", "/search/results") if target.ends_with("q=compatibility+test") => {
            "<main><h1>Search results</h1><p>GET query encoding confirmed.</p></main>"
        }
        (_, "/search/results") => {
            "<main><h1>Search results</h1><p>GET query was missing or incorrectly encoded.</p></main>"
        }
        ("POST", "/search/submit") if post_body == "q=compatibility+test" => {
            "<main><h1>Search submitted</h1><p>URL-encoded POST body confirmed.</p></main>"
        }
        ("POST", "/search/submit") => {
            "<main><h1>Search submission error</h1><p>POST body was missing or incorrectly encoded.</p></main>"
        }
        (_, "/client-route") => {
            "<main><h1>Server route</h1><p>Client-side navigation should prevent this request.</p></main>"
        }
        _ => "<main><h1>Fixture not found</h1></main>",
    };
    let status = if path == "/missing" {
        "404 Not Found"
    } else {
        "200 OK"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

struct ReportRow {
    category: String,
    result: String,
    elapsed_ms: f64,
    details: String,
}

fn row(
    rows: &mut Vec<ReportRow>,
    category: &str,
    passed: bool,
    elapsed: Duration,
    details: impl Into<String>,
) {
    rows.push(ReportRow {
        category: category.into(),
        result: if passed { "PASS" } else { "FAIL" }.into(),
        elapsed_ms: elapsed.as_secs_f64() * 1_000.0,
        details: details.into(),
    });
}

async fn check_fixture(
    client: &HttpClient,
    origin: &str,
    path: &str,
    expected: &str,
    category: &str,
    rows: &mut Vec<ReportRow>,
) -> Result<(String, webmorph::dom::Document, usize), String> {
    let url = HttpClient::parse_url(&format!("{origin}{path}"))
        .map_err(|error| format!("invalid local fixture URL: {error}"))?;
    let started = Instant::now();
    let (final_url, source) = timeout(
        Duration::from_secs(8),
        client.get_text_with_final_url_cancellable(&url, CancellationToken::new()),
    )
    .await
    .map_err(|_| format!("local fixture request hung for {path}"))?
    .map_err(|error| format!("local fixture request failed for {path}: {error}"))?;
    let loaded = Instant::now();
    let document = html::parse(final_url.as_str(), &source);
    let rendered = transform(&document);
    let meaningful = rendered.lines.join(" ").contains(expected);
    row(
        rows,
        category,
        meaningful,
        started.elapsed(),
        format!(
            "HTTP success; {} source bytes; parse+transform {:.2} ms; {} rendered bytes",
            source.len(),
            loaded.elapsed().as_secs_f64() * 1_000.0,
            rendered.lines.iter().map(String::len).sum::<usize>(),
        ),
    );
    Ok((
        source,
        document,
        rendered.lines.iter().map(String::len).sum(),
    ))
}

async fn run_local() -> Result<String, String> {
    let fixture_server = FixtureServer::start().await?;
    let mut rows = Vec::new();
    let client_started = Instant::now();
    let client = HttpClient::new(HttpConfig {
        request_timeout: Duration::from_secs(8),
        allow_private_network: true,
        ..HttpConfig::default()
    })
    .map_err(|error| format!("lightweight client initialization failed: {error}"))?;
    let startup_ms = client_started.elapsed().as_secs_f64() * 1_000.0;
    let mut rendered_bytes = 0usize;

    for (path, expected, category) in [
        ("/static", "Static page", "Static HTML"),
        ("/documentation", "Reference Guide", "Documentation"),
        ("/navigation", "Nested navigation", "Complex navigation"),
        ("/search", "Search", "Search form"),
        ("/tables", "Service status", "Tables"),
    ] {
        let (_, _, bytes) = check_fixture(
            &client,
            &fixture_server.origin,
            path,
            expected,
            category,
            &mut rows,
        )
        .await?;
        rendered_bytes = rendered_bytes.saturating_add(bytes);
    }

    let (_, mapped_page, _) = check_fixture(
        &client,
        &fixture_server.origin,
        "/navigation",
        "Nested navigation",
        "Interactive mapping",
        &mut rows,
    )
    .await?;
    let unique_ids = mapped_page
        .interactive
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let links = mapped_page
        .interactive
        .iter()
        .filter(|item| item.href.is_some())
        .count();
    let mapping_valid = !mapped_page.interactive.is_empty()
        && unique_ids.len() == mapped_page.interactive.len()
        && links >= 4;
    if let Some(mapping_row) = rows
        .iter_mut()
        .find(|entry| entry.category == "Interactive mapping")
    {
        mapping_row.result = if mapping_valid { "PASS" } else { "FAIL" }.into();
        mapping_row.details = format!(
            "{} unique stable IDs; {links} resolved links",
            unique_ids.len()
        );
    }

    let (_, navigation, _) = check_fixture(
        &client,
        &fixture_server.origin,
        "/documentation",
        "Reference Guide",
        "Link mapping",
        &mut rows,
    )
    .await?;
    let next_chapter = navigation
        .interactive
        .iter()
        .find(|item| item.label == "Next chapter")
        .and_then(|item| item.href.as_deref())
        .ok_or("documentation link did not map to an interactive target")?;
    let next_url = HttpClient::parse_url(next_chapter)
        .map_err(|error| format!("resolved link URL is invalid: {error}"))?;
    let started = Instant::now();
    let next_text = client
        .get_text(&next_url)
        .await
        .map_err(|error| format!("link navigation failed: {error}"))?;
    row(
        &mut rows,
        "Link navigation",
        next_text.contains("Link navigation reached this page"),
        started.elapsed(),
        format!(
            "mapped ID {}; destination loaded",
            navigation
                .interactive
                .iter()
                .find(|item| item.label == "Next chapter")
                .unwrap()
                .id
        ),
    );

    let (_, search, _) = check_fixture(
        &client,
        &fixture_server.origin,
        "/search",
        "Search",
        "Search controls",
        &mut rows,
    )
    .await?;
    let search_form = search
        .forms
        .first()
        .ok_or("search form was not normalized")?;
    let query = search_form
        .controls
        .iter()
        .find(|control| control.name.as_deref() == Some("q"))
        .ok_or("search input mapping was not found")?;
    let action = HttpClient::parse_url(&search_form.action)
        .map_err(|error| format!("search form action is invalid: {error}"))?;
    let get_url =
        HttpClient::append_form_query(&action, &[("q".into(), "compatibility test".into())]);
    let form_started = Instant::now();
    let get_result = client
        .get_text(&get_url)
        .await
        .map_err(|error| format!("GET search submission failed: {error}"))?;
    let post_url = HttpClient::resolve_url(
        &HttpClient::parse_url(&fixture_server.origin).unwrap(),
        "/search/submit",
    )
    .map_err(|error| format!("POST fixture URL is invalid: {error}"))?;
    let post_result = client
        .submit_form_cancellable(
            &post_url,
            vec![("q".into(), "compatibility test".into())],
            CancellationToken::new(),
        )
        .await
        .map_err(|error| format!("POST search submission failed: {error}"))?;
    row(
        &mut rows,
        "Form behavior",
        query.required
            && get_result.contains("GET query encoding confirmed")
            && post_result.1.contains("URL-encoded POST body confirmed"),
        form_started.elapsed(),
        format!(
            "mapped required field {}; GET and URL-encoded POST returned local pages",
            query.interactive_id
        ),
    );

    let (shell_source, shell, _) = check_fixture(
        &client,
        &fixture_server.origin,
        "/javascript-app",
        "No readable page content",
        "JavaScript application shell",
        &mut rows,
    )
    .await?;
    let shell_assessment = assess(&shell_source, &shell);
    let shell_is_fallback_candidate = shell_assessment == PageAssessment::JavaScriptShell;
    row(
        &mut rows,
        "Lightweight fallback detection",
        shell_is_fallback_candidate,
        Duration::ZERO,
        format!("assessment: {shell_assessment:?}; explicit Chromium fallback eligible"),
    );

    let warm_url = HttpClient::parse_url(&format!("{}/static", fixture_server.origin))
        .map_err(|error| error.to_string())?;
    client
        .get_text(&warm_url)
        .await
        .map_err(|error| format!("warm-up navigation failed: {error}"))?;
    let before_rss = resident_set_bytes();
    let repeat_started = Instant::now();
    for _ in 0..16 {
        let _ = client
            .get_text(&warm_url)
            .await
            .map_err(|error| format!("repeated navigation failed: {error}"))?;
    }
    let middle_rss = resident_set_bytes();
    for _ in 0..16 {
        let _ = client
            .get_text(&warm_url)
            .await
            .map_err(|error| format!("second repeated navigation batch failed: {error}"))?;
    }
    let after_rss = resident_set_bytes();
    let first_growth = rss_delta_kib(before_rss, middle_rss);
    let second_growth = rss_delta_kib(middle_rss, after_rss);
    let growth = match (first_growth, second_growth) {
        (Some(first), Some(second)) => format!(
            "estimated RSS deltas after two warmed 16-request batches: {first:+} KiB, then {second:+} KiB"
        ),
        _ => "process RSS measurement unavailable on this platform".into(),
    };
    let continued_growth = second_growth.is_some_and(|delta| delta > 0);
    row(
        &mut rows,
        "Repeated navigation/resource check",
        fixture_server.requests.load(Ordering::Relaxed) >= 44,
        repeat_started.elapsed(),
        format!(
            "32 repeated requests completed; {growth}; {}",
            if continued_growth {
                "RSS rose during the second batch; investigate if repeatable"
            } else {
                "no positive RSS growth in the second batch"
            }
        ),
    );

    let browser_result = run_optional_chromium(&fixture_server.origin).await;
    rows.push(browser_result);

    let request_count = fixture_server.requests.load(Ordering::Relaxed);
    fixture_server.close().await;
    Ok(render_report(
        "Deterministic local compatibility suite",
        startup_ms,
        request_count,
        rendered_bytes,
        rows,
    ))
}

#[cfg(feature = "browser-engine")]
async fn run_optional_chromium(origin: &str) -> ReportRow {
    use webmorph::browser::{BrowserBackend, BrowserInteraction, ChromiumConfig, ChromiumEngine};

    let started = Instant::now();
    let Ok(engine) = ChromiumEngine::detect() else {
        return ReportRow {
            category: "Chromium dynamic interaction".into(),
            result: "SKIP".into(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
            details: "optional browser executable not installed".into(),
        };
    };
    let engine = engine.with_config(ChromiumConfig {
        allow_private_network: true,
        navigation_timeout: Duration::from_secs(10),
        max_sessions: 1,
        ..ChromiumConfig::default()
    });
    let startup_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let token = CancellationToken::new();
    let navigation_started = Instant::now();
    let navigation = timeout(
        Duration::from_secs(15),
        engine.navigate(90_017, &format!("{origin}/javascript-app"), token.clone()),
    )
    .await;
    let navigation_ms = navigation_started.elapsed().as_secs_f64() * 1_000.0;
    let interactions_started = Instant::now();
    let result = async {
        let (mut document, _) = navigation
            .map_err(|_| "Chromium navigation timed out".to_owned())?
            .map_err(|error| format!("Chromium navigation failed: {error}"))?;
        if !transform(&document)
            .lines
            .join(" ")
            .contains("JavaScript ready")
        {
            return Err("rendered JavaScript content was missing".to_owned());
        }
        let update = document
            .interactive
            .iter()
            .find(|item| item.label == "Update")
            .map(|item| item.id.clone())
            .ok_or_else(|| {
                format!(
                    "dynamic button was not mapped; controls seen: {}",
                    document
                        .interactive
                        .iter()
                        .map(|item| item.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        (document, _) = engine
            .interact(
                90_017,
                BrowserInteraction::Click {
                    interactive_id: update,
                },
                token.clone(),
            )
            .await
            .map_err(|error| format!("dynamic click failed: {error}"))?;
        if !transform(&document)
            .lines
            .join(" ")
            .contains("Dynamic content updated")
        {
            return Err("dynamic click did not update rendered content".to_owned());
        }
        let route = document
            .interactive
            .iter()
            .find(|item| item.label == "Open client route")
            .map(|item| item.id.clone())
            .ok_or("client-side navigation link was not mapped")?;
        let (document, final_url) = engine
            .interact(
                90_017,
                BrowserInteraction::Click {
                    interactive_id: route,
                },
                token.clone(),
            )
            .await
            .map_err(|error| format!("client navigation action failed: {error}"))?;
        if !final_url.ends_with("/client-route")
            || !transform(&document)
                .lines
                .join(" ")
                .contains("Client route rendered")
        {
            return Err("client-side route did not update browser and TUI state".to_owned());
        }
        Ok::<_, String>(())
    }
    .await;
    engine.close_session(90_017).await;
    let mut result = result;
    let mut lifecycle_rounds = 0usize;
    if result.is_ok() {
        for session_id in 90_018..90_020 {
            let navigation = timeout(
                Duration::from_secs(15),
                engine.navigate(
                    session_id,
                    &format!("{origin}/javascript-app"),
                    CancellationToken::new(),
                ),
            )
            .await;
            engine.close_session(session_id).await;
            lifecycle_rounds += 1;
            if !matches!(navigation, Ok(Ok(_))) || engine.active_session_count().await != 0 {
                result = Err(format!(
                    "browser lifecycle cleanup failed in round {lifecycle_rounds}"
                ));
                break;
            }
        }
    }
    let sessions_left = engine.active_session_count().await;
    let passed = result.is_ok() && sessions_left == 0;
    ReportRow {
        category: "Chromium dynamic interaction".into(),
        result: if passed { "PASS" } else { "FAIL" }.into(),
        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        details: result.err().unwrap_or_else(|| {
            format!(
                "browser detection {:.2} ms; launch+initial navigation {:.2} ms; actions {:.2} ms; dynamic update and client route passed; {} repeated create/close rounds; sessions after close: {sessions_left}",
                startup_ms,
                navigation_ms,
                interactions_started.elapsed().as_secs_f64() * 1_000.0,
                lifecycle_rounds,
            )
        }),
    }
}

#[cfg(not(feature = "browser-engine"))]
async fn run_optional_chromium(_origin: &str) -> ReportRow {
    ReportRow {
        category: "Chromium dynamic interaction".into(),
        result: "SKIP".into(),
        elapsed_ms: 0.0,
        details: "build with --features browser-engine to enable optional browser verification"
            .into(),
    }
}

async fn run_live_smoke() -> Vec<ReportRow> {
    let Ok(client) = HttpClient::new(HttpConfig {
        request_timeout: Duration::from_secs(12),
        ..HttpConfig::default()
    }) else {
        return vec![ReportRow {
            category: "Live smoke client initialization".into(),
            result: "FAIL".into(),
            elapsed_ms: 0.0,
            details: "could not initialize lightweight HTTP client".into(),
        }];
    };
    let mut rows = Vec::new();
    for (site, url) in [
        ("example.com", "https://example.com/"),
        ("Rust language site", "https://www.rust-lang.org/"),
    ] {
        let started = Instant::now();
        let result = async {
            let url = HttpClient::parse_url(url).map_err(|error| error.to_string())?;
            let (final_url, source) = client
                .get_text_with_final_url_cancellable(&url, CancellationToken::new())
                .await
                .map_err(|error| format_live_error(&error))?;
            let document = html::parse(final_url.as_str(), &source);
            let page = transform(&document);
            let assessment = assess(&source, &document);
            let useful = assessment == PageAssessment::Useful && !page.lines.is_empty();
            Ok::<_, String>((
                source.len(),
                page.lines.iter().map(String::len).sum::<usize>(),
                assessment,
                useful,
            ))
        }
        .await;
        let (status, details) = match result {
            Ok((source_bytes, rendered_bytes, assessment, useful)) => (
                if useful { "PASS" } else { "REACHED" },
                format!(
                    "assessment {assessment:?}; {source_bytes} source bytes; {rendered_bytes} rendered bytes"
                ),
            ),
            Err(message) if message.starts_with("NETWORK UNAVAILABLE:") => {
                ("NETWORK UNAVAILABLE", message)
            }
            Err(message) if message.starts_with("SITE RESTRICTED:") => ("SITE RESTRICTED", message),
            Err(message) if message.starts_with("SITE CONTENT:") => ("SITE CONTENT", message),
            Err(message) if message.starts_with("REMOTE REDIRECT/POLICY:") => {
                ("REMOTE RESPONSE", message)
            }
            Err(message) if message.starts_with("REMOTE HTTP RESPONSE:") => {
                ("REMOTE RESPONSE", message)
            }
            Err(message) => ("IMPLEMENTATION FAILURE", message),
        };
        rows.push(ReportRow {
            category: format!("Live smoke: {site}"),
            result: status.into(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
            details,
        });
    }
    rows
}

fn format_live_error(error: &WebMorphError) -> String {
    match error {
        WebMorphError::Dns(_)
        | WebMorphError::Connection(_)
        | WebMorphError::Tls(_)
        | WebMorphError::Timeout => {
            format!("NETWORK UNAVAILABLE: {error}")
        }
        WebMorphError::AccessDenied
        | WebMorphError::AuthenticationRequired
        | WebMorphError::RateLimited => {
            format!("SITE RESTRICTED: {error}")
        }
        WebMorphError::Redirect(_) => format!("REMOTE REDIRECT/POLICY: {error}"),
        WebMorphError::HttpStatus(_) => format!("REMOTE HTTP RESPONSE: {error}"),
        WebMorphError::UnsupportedContent(_) | WebMorphError::ResponseTooLarge { .. } => {
            format!("SITE CONTENT: {error}")
        }
        other => other.to_string(),
    }
}

fn render_report(
    title: &str,
    client_startup_ms: f64,
    requests: usize,
    rendered_bytes: usize,
    rows: Vec<ReportRow>,
) -> String {
    let rust_version = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into());
    let mut output = format!(
        "# {title}\n\n- Platform: `{}` / `{}`\n- Rust: `{rust_version}`\n- Lightweight client initialization: {:.3} ms\n- Local HTTP requests served: {requests}\n- Retained rendered text measured across fixture pages: {rendered_bytes} bytes\n\n| Category | Result | Elapsed (ms) | Details |\n| --- | --- | ---: | --- |\n",
        env::consts::OS,
        env::consts::ARCH,
        client_startup_ms,
    );
    for row in rows {
        let details = row.details.replace('|', "\\|").replace('\n', " ");
        output.push_str(&format!(
            "| {} | {} | {:.3} | {} |\n",
            row.category, row.result, row.elapsed_ms, details
        ));
    }
    output.push_str(
        "\nThis report uses deterministic loopback fixtures. Live-site results are separate and are recorded only when `--live-smoke` is requested. Chromium skips when the feature or executable is unavailable. RSS is sampled from `/proc` on Linux only; elsewhere the report states that process RSS is unavailable.\n",
    );
    output
}

#[cfg(target_os = "linux")]
fn resident_set_bytes() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let resident_kib = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?;
    Some(resident_kib.saturating_mul(1024))
}

fn rss_delta_kib(before: Option<u64>, after: Option<u64>) -> Option<i64> {
    Some((after? as i64 - before? as i64) / 1024)
}

#[cfg(not(target_os = "linux"))]
fn resident_set_bytes() -> Option<u64> {
    None
}

fn parse_args() -> (Option<PathBuf>, bool) {
    let mut output = None;
    let mut live = false;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--output" => output = args.next().map(PathBuf::from),
            "--live-smoke" => live = true,
            "--help" | "-h" => {
                println!("Usage: compatibility-report [--output PATH] [--live-smoke]");
                std::process::exit(0);
            }
            unknown => {
                eprintln!("Unknown argument: {unknown}");
                std::process::exit(2);
            }
        }
    }
    (output, live)
}

#[tokio::main]
async fn main() {
    let (output_path, live) = parse_args();
    let report = match timeout(Duration::from_secs(120), run_local()).await {
        Ok(Ok(report)) => report,
        Ok(Err(error)) => {
            eprintln!("compatibility harness failed: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("compatibility harness exceeded its 120 second timeout");
            std::process::exit(1);
        }
    };
    let report = if live {
        let live_rows = run_live_smoke().await;
        let mut output = report;
        output.push_str("\n## Optional live smoke tests\n\n");
        output.push_str("| Site | Result | Elapsed (ms) | Details |\n| --- | --- | ---: | --- |\n");
        for row in live_rows {
            output.push_str(&format!(
                "| {} | {} | {:.3} | {} |\n",
                row.category.replace("Live smoke: ", ""),
                row.result,
                row.elapsed_ms,
                row.details.replace('|', "\\|").replace('\n', " "),
            ));
        }
        output
    } else {
        report
    };
    let failed = report.contains("| FAIL |") || report.contains("| IMPLEMENTATION FAILURE |");
    if let Some(path) = output_path {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            && let Err(error) = fs::create_dir_all(parent)
        {
            eprintln!("could not create report directory: {error}");
            std::process::exit(1);
        }
        if let Err(error) = fs::write(&path, &report) {
            eprintln!("could not write report {}: {error}", path.display());
            std::process::exit(1);
        }
        println!("Compatibility report written to {}", path.display());
    } else {
        print!("{report}");
    }
    if failed {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{ReportRow, format_live_error, render_report};
    use std::time::Duration;
    use webmorph::error::WebMorphError;

    #[test]
    fn live_smoke_classifies_availability_and_site_restrictions_separately() {
        assert!(
            format_live_error(&WebMorphError::Dns("offline".into()))
                .starts_with("NETWORK UNAVAILABLE:")
        );
        assert!(format_live_error(&WebMorphError::AccessDenied).starts_with("SITE RESTRICTED:"));
        assert!(
            format_live_error(&WebMorphError::UnsupportedContent("image/png".into()))
                .starts_with("SITE CONTENT:")
        );
    }

    #[test]
    fn report_records_only_supplied_observations() {
        let report = render_report(
            "Test report",
            1.0,
            2,
            3,
            vec![ReportRow {
                category: "Static HTML".into(),
                result: "PASS".into(),
                elapsed_ms: Duration::from_millis(4).as_secs_f64() * 1_000.0,
                details: "fixture completed".into(),
            }],
        );
        assert!(report.contains("Static HTML | PASS | 4.000"));
        assert!(report.contains("linux") || report.contains("windows") || report.contains("macos"));
        assert!(!report.contains("100%"));
    }
}
