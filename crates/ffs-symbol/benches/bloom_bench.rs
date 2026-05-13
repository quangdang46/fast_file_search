//! Microbenchmarks for the bloom filter and identifier extractor.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use ffs_symbol::bloom::{extract_identifiers, BloomFilter};

fn bench_insert_and_contains(c: &mut Criterion) {
    let identifiers: Vec<String> = (0..1024).map(|i| format!("symbol_{i}")).collect();

    let mut group = c.benchmark_group("bloom_insert");
    for size in [128usize, 1024, 8192] {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let mut filter = BloomFilter::new(size, 0.01);
                for ident in identifiers.iter().take(size) {
                    filter.insert(black_box(ident));
                }
                filter
            });
        });
    }
    group.finish();

    let mut filter = BloomFilter::new(1024, 0.01);
    for ident in &identifiers {
        filter.insert(ident);
    }
    let queries: Vec<String> = (0..2048)
        .map(|i| {
            if i % 2 == 0 {
                identifiers[i % identifiers.len()].clone()
            } else {
                format!("missing_{i}")
            }
        })
        .collect();

    c.bench_function("bloom_contains_mixed", |b| {
        b.iter(|| {
            let mut hits = 0u64;
            for q in &queries {
                if filter.contains(black_box(q)) {
                    hits += 1;
                }
            }
            hits
        });
    });
}

fn bench_identifier_extraction(c: &mut Criterion) {
    let body = r#"
        // SPDX-License-Identifier: MIT
        use std::collections::HashMap;
        pub fn build_index(items: &[String]) -> HashMap<String, usize> {
            let mut out = HashMap::new();
            for (idx, name) in items.iter().enumerate() {
                out.insert(name.clone(), idx);
            }
            out
        }
        struct Worker { id: u64, name: String, queue: Vec<String> }
        impl Worker {
            pub fn run(&mut self, payload: &str) -> Result<(), String> {
                self.queue.push(payload.to_string());
                Ok(())
            }
        }
    "#;
    let buf = body.repeat(64);
    let mut group = c.benchmark_group("identifier_extraction");
    group.throughput(Throughput::Bytes(buf.len() as u64));
    group.bench_function("typical_rust", |b| {
        b.iter(|| {
            let mut count = 0usize;
            for ident in extract_identifiers(black_box(&buf)) {
                count += ident.len();
            }
            count
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_insert_and_contains,
    bench_identifier_extraction
);
criterion_main!(benches);
