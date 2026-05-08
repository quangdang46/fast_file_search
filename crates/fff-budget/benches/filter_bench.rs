//! Microbenchmarks for filter strategies.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fff_budget::filter::{AggressiveFilter, FilterStrategy, MinimalFilter, NoFilter};

fn make_source(size: usize) -> String {
    let block = r#"// SPDX-License-Identifier: MIT
/* multi-line
 * block comment
 */
use std::collections::HashMap;
fn process(items: &[String]) -> HashMap<String, usize> {
    // inline comment
    let mut out = HashMap::new();
    for (idx, name) in items.iter().enumerate() {
        out.insert(name.clone(), idx);
    }


    out
}

"#;
    let mut s = String::with_capacity(size);
    while s.len() < size {
        s.push_str(block);
    }
    s
}

fn bench_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter_apply");
    for size in [4096usize, 65_536, 524_288] {
        let body = make_source(size);
        group.throughput(Throughput::Bytes(body.len() as u64));
        group.bench_with_input(BenchmarkId::new("none", size), &body, |b, body| {
            let f = NoFilter;
            b.iter(|| f.apply(black_box(body)));
        });
        group.bench_with_input(BenchmarkId::new("minimal", size), &body, |b, body| {
            let f = MinimalFilter;
            b.iter(|| f.apply(black_box(body)));
        });
        group.bench_with_input(BenchmarkId::new("aggressive", size), &body, |b, body| {
            let f = AggressiveFilter;
            b.iter(|| f.apply(black_box(body)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_filter);
criterion_main!(benches);
