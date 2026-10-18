use std::{hint::black_box, time::Instant};

use webmorph::{
    html,
    transform::{layout_components, transform},
};

fn main() {
    let mut source = String::with_capacity(1_000_000);
    source.push_str("<html><head><title>Benchmark</title></head><body><main>");
    for index in 0..2_000 {
        source.push_str(&format!(
            "<article><h2>Section {index}</h2><p>Representative paragraph with enough words to wrap at terminal widths.</p><a href='/item/{index}'>Open item {index}</a><ul><li>First</li><li>Second</li></ul></article>"
        ));
        if index % 2 == 0 {
            source.push_str(&format!(
                "<form><label for='q{index}'>Search {index}</label><input id='q{index}' name='q' type='search'><button name='go'>Go</button></form>"
            ));
        }
    }
    source.push_str("</main></body></html>");
    let url = "https://example.test/docs";
    let iterations = 30;

    let start = Instant::now();
    for _ in 0..iterations {
        black_box(html::parse(url, black_box(&source)));
    }
    report("HTML parse", start.elapsed(), iterations);

    let document = html::parse(url, &source);
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(transform(black_box(&document)));
    }
    report("document transform", start.elapsed(), iterations);

    let page = transform(&document);
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(layout_components(black_box(&page.root), 80));
    }
    report("terminal layout", start.elapsed(), iterations);
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(
            page.lines
                .iter()
                .skip(200)
                .take(20)
                .cloned()
                .collect::<Vec<_>>(),
        );
    }
    report("cached visible-window copy", start.elapsed(), iterations);
    eprintln!(
        "fixture: {} bytes; 2,000 paragraphs, 2,000 links, 1,000 labeled forms",
        source.len()
    );
}

fn report(stage: &str, elapsed: std::time::Duration, iterations: u32) {
    eprintln!(
        "{stage}: {:.4} ms/iteration ({iterations} iterations; {:.2} ms total)",
        elapsed.as_secs_f64() * 1_000.0 / f64::from(iterations),
        elapsed.as_secs_f64() * 1_000.0,
    );
}
