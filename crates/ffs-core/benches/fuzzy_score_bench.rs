//! Benchmark neo_frizbee fuzzy path matching (hot path behind score.rs / fuzzy_grep).
//!
//! Pure frizbee workload over synthetic path haystacks so we can compare
//! frizbee versions without filesystem noise.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use neo_frizbee::{Config, Matcher, Scoring, match_list, match_list_parallel};

fn make_paths(n: usize) -> Vec<String> {
    let templates = [
        "crates/ffs-core/src/file_picker.rs",
        "crates/ffs-core/src/score.rs",
        "crates/ffs-core/src/grep/grep.rs",
        "crates/ffs-core/src/constraints.rs",
        "crates/ffs-mcp/src/server.rs",
        "crates/ffs-cli/src/commands/grep.rs",
        "crates/ffs-query-parser/src/parser.rs",
        "crates/ffs-engine/src/dispatch.rs",
        "crates/ffs-symbol/src/outline.rs",
        "crates/ffs-budget/src/filter.rs",
    ];
    (0..n)
        .map(|i| {
            let t = templates[i % templates.len()];
            format!("proj{i}/{t}")
        })
        .collect()
}

fn bench_fuzzy_score(c: &mut Criterion) {
    let mut group = c.benchmark_group("fuzzy_score_frizbee");
    group.sample_size(30);
    group.warm_up_time(std::time::Duration::from_secs(1));
    group.measurement_time(std::time::Duration::from_secs(2));

    let needles = ["score", "file_picker", "grep", "constraints", "parser"];
    // Keep sizes modest for CI-friendly runs; 50k is enough to show scaling.
    let sizes = [1_000usize, 10_000, 50_000];

    for &n in &sizes {
        let paths = make_paths(n);
        let haystacks: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

        for needle in needles {
            let id = format!("n{n}/{needle}");

            group.bench_with_input(BenchmarkId::new("match_list", &id), &haystacks, |b, hs| {
                let config = Config {
                    max_typos: Some(2),
                    scoring: Scoring {
                        exact_match_bonus: 100,
                        ..Scoring::default()
                    },
                    ..Config::default()
                };
                b.iter(|| {
                    let mut matcher = Matcher::new(black_box(needle), black_box(&config));
                    black_box(matcher.match_list(black_box(hs)))
                });
            });

            group.bench_with_input(
                BenchmarkId::new("match_list_fn", &id),
                &haystacks,
                |b, hs| {
                    let config = Config {
                        max_typos: Some(2),
                        scoring: Scoring {
                            exact_match_bonus: 100,
                            ..Scoring::default()
                        },
                        ..Config::default()
                    };
                    b.iter(|| {
                        black_box(match_list(
                            black_box(needle),
                            black_box(hs),
                            black_box(&config),
                        ))
                    });
                },
            );

            if n >= 10_000 {
                group.bench_with_input(
                    BenchmarkId::new("match_list_parallel_8", &id),
                    &haystacks,
                    |b, hs| {
                        let config = Config {
                            max_typos: Some(2),
                            scoring: Scoring {
                                exact_match_bonus: 100,
                                ..Scoring::default()
                            },
                            ..Config::default()
                        };
                        b.iter(|| {
                            black_box(match_list_parallel(
                                black_box(needle),
                                black_box(hs),
                                black_box(&config),
                                8,
                            ))
                        });
                    },
                );
            }
        }
    }

    group.finish();
}

criterion_group!(benches, bench_fuzzy_score);
criterion_main!(benches);
