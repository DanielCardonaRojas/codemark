use codetours_server::highlight::{get_cache, highlight};
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn bench_highlight(c: &mut Criterion) {
    let language = "rust";
    let content = r#"
        pub fn fibonacci(n: u32) -> u32 {
            match n {
                0 => 1,
                1 => 1,
                _ => fibonacci(n - 1) + fibonacci(n - 2),
            }
        }
    "#;

    // Warm up the cache
    let _ = highlight(language, content);

    c.bench_function("highlight_cached", |b| {
        b.iter(|| highlight(black_box(language), black_box(content)));
    });

    c.bench_function("highlight_uncached", |b| {
        b.iter(|| {
            // clear the cache each iteration to simulate a miss
            get_cache().clear();
            highlight(black_box(language), black_box(content))
        });
    });
}

criterion_group!(benches, bench_highlight);
criterion_main!(benches);
