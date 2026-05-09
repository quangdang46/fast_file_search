//! Microbenchmarks for symbol extraction and the symbol index.

use std::path::PathBuf;
use std::time::SystemTime;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use fff_symbol::symbol_index::SymbolIndex;

const RUST_SOURCE: &str = r#"
use std::collections::HashMap;

pub fn build_index(items: &[String]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for (idx, name) in items.iter().enumerate() {
        out.insert(name.clone(), idx);
    }
    out
}

pub struct Worker {
    pub id: u64,
    pub name: String,
    pub queue: Vec<String>,
}

impl Worker {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            name: String::new(),
            queue: Vec::new(),
        }
    }

    pub fn run(&mut self, payload: &str) -> Result<(), String> {
        self.queue.push(payload.to_string());
        Ok(())
    }
}

pub trait Identifiable {
    fn identifier(&self) -> u64;
}

impl Identifiable for Worker {
    fn identifier(&self) -> u64 {
        self.id
    }
}
"#;

fn bench_index_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("symbol_index_file");
    group.throughput(Throughput::Bytes(RUST_SOURCE.len() as u64));
    group.bench_function("rust_small", |b| {
        b.iter_with_setup(SymbolIndex::new, |idx| {
            let path = PathBuf::from("/tmp/scry_bench.rs");
            idx.index_file(
                black_box(&path),
                SystemTime::UNIX_EPOCH,
                black_box(RUST_SOURCE),
            )
        });
    });
    group.finish();
}

fn bench_symbol_index_lookup(c: &mut Criterion) {
    let index = SymbolIndex::new();
    let mtime = SystemTime::UNIX_EPOCH;
    for i in 0..1000 {
        let path = PathBuf::from(format!("/tmp/scry_bench_{i}.rs"));
        index.index_file(&path, mtime, RUST_SOURCE);
    }

    c.bench_function("symbol_index_lookup_exact_hit", |b| {
        b.iter(|| index.lookup_exact(black_box("Worker")));
    });
    c.bench_function("symbol_index_lookup_exact_miss", |b| {
        b.iter(|| index.lookup_exact(black_box("DefinitelyNotPresent")));
    });
    c.bench_function("symbol_index_lookup_prefix", |b| {
        b.iter(|| index.lookup_prefix(black_box("Wor")));
    });
}

criterion_group!(benches, bench_index_file, bench_symbol_index_lookup);
criterion_main!(benches);
