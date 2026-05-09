//! Microbenchmarks for query classification, ranking, and end-to-end dispatch.

use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fff_engine::classify::classify_query;
use fff_engine::ranking::{score_one, RankInputs};
use fff_engine::{Engine, EngineConfig};
use tempfile::TempDir;

fn bench_classify(c: &mut Criterion) {
    let cwd = PathBuf::from(".");
    let mut group = c.benchmark_group("classify_query");
    let queries = [
        "src/main.rs",
        "src/main.rs:42",
        "**/*.ts",
        "build_index",
        "BloomFilter",
        "how does the bloom filter handle mtime invalidation",
    ];
    for q in queries {
        group.bench_function(q, |b| {
            b.iter(|| classify_query(black_box(q), black_box(&cwd)));
        });
    }
    group.finish();
}

fn bench_score_one(c: &mut Criterion) {
    c.bench_function("score_one_definition", |b| {
        b.iter(|| {
            score_one(black_box(RankInputs {
                def_weight: 100,
                frecency_score: 50,
                exact: true,
                is_definition: true,
                in_comment: false,
            }))
        });
    });
    c.bench_function("score_one_usage_in_comment", |b| {
        b.iter(|| {
            score_one(black_box(RankInputs {
                def_weight: 0,
                frecency_score: 5,
                exact: false,
                is_definition: false,
                in_comment: true,
            }))
        });
    });
}

fn build_fixture_repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    for i in 0..32 {
        let dir = root.join(format!("mod_{i:02}"));
        std::fs::create_dir_all(&dir).unwrap();
        for j in 0..8 {
            let file = dir.join(format!("file_{j}.rs"));
            let body = format!(
                r#"
pub fn worker_{i}_{j}(payload: &str) -> String {{
    payload.to_string()
}}

pub struct Item_{i}_{j} {{
    pub id: u64,
}}

impl Item_{i}_{j} {{
    pub fn run(&self) -> u64 {{
        self.id
    }}
}}
"#,
                i = i,
                j = j,
            );
            std::fs::write(file, body).unwrap();
        }
    }
    tmp
}

fn bench_engine_dispatch(c: &mut Criterion) {
    let tmp = build_fixture_repo();
    let cfg = EngineConfig::default();
    let engine = Engine::new(cfg);
    let report = engine.index(tmp.path());
    let _ = report;

    c.bench_function("engine_dispatch_symbol", |b| {
        b.iter(|| engine.dispatch(black_box("worker_05_3"), black_box(tmp.path())));
    });
    c.bench_function("engine_dispatch_glob", |b| {
        b.iter(|| engine.dispatch(black_box("**/*.rs"), black_box(tmp.path())));
    });
    c.bench_function("engine_dispatch_filepath", |b| {
        b.iter(|| engine.dispatch(black_box("mod_03/file_2.rs"), black_box(tmp.path())));
    });
    c.bench_function("engine_dispatch_concept", |b| {
        b.iter(|| {
            engine.dispatch(
                black_box("how does the worker handle payloads"),
                black_box(tmp.path()),
            )
        });
    });
}

criterion_group!(
    benches,
    bench_classify,
    bench_score_one,
    bench_engine_dispatch,
);
criterion_main!(benches);
