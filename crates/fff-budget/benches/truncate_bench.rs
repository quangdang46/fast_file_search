//! Microbenchmarks for smart truncation and footer-preserving trimming.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fff_budget::truncate::{apply_preserving_footer, smart_truncate};

fn make_input(lines: usize) -> String {
    let mut s = String::new();
    for i in 0..lines {
        if i % 7 == 0 {
            s.push_str(&format!("// comment line {i}\n"));
        } else {
            s.push_str(&format!("line {i} of mostly normal text content\n"));
        }
    }
    s
}

fn bench_smart_truncate(c: &mut Criterion) {
    let inputs: Vec<(usize, String)> = [128usize, 1024, 8192]
        .into_iter()
        .map(|n| (n, make_input(n)))
        .collect();

    let mut group = c.benchmark_group("smart_truncate");
    for (n, body) in &inputs {
        group.throughput(Throughput::Bytes(body.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(n),
            &(*n, body),
            |b, (_n, body)| {
                let limit = body.len() / 2;
                b.iter(|| smart_truncate(black_box(body), black_box(limit)));
            },
        );
    }
    group.finish();
}

fn bench_apply_preserving_footer(c: &mut Criterion) {
    let body = make_input(2048);
    let footer = "[truncated, see file for the rest]\n";
    let mut group = c.benchmark_group("apply_preserving_footer");
    group.throughput(Throughput::Bytes(body.len() as u64));
    for budget in [256usize, 4096, 65536] {
        group.bench_with_input(
            BenchmarkId::from_parameter(budget),
            &budget,
            |b, &budget| {
                b.iter(|| {
                    let mut buf = String::with_capacity(budget + footer.len());
                    apply_preserving_footer(
                        black_box(&mut buf),
                        black_box(budget),
                        black_box(footer),
                        |out, payload_budget| {
                            let take = body.len().min(payload_budget);
                            out.push_str(&body[..take]);
                            body.len()
                        },
                    )
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_smart_truncate, bench_apply_preserving_footer);
criterion_main!(benches);
