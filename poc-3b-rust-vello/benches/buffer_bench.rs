use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use poc_3b_rust_vello::{LineIndex, PieceTable, TextBuffer};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn gen_text(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * 82);
    for i in 0..lines {
        let line = format!("fn function_{i:06}() {{ /* body */ return {i}; }}\n");
        out.extend_from_slice(line.as_bytes());
    }
    out
}

// ── LineIndex benchmarks ─────────────────────────────────────────────────────

fn bench_line_index_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("LineIndex/build");

    for lines in [1_000, 10_000, 100_000, 1_300_000] {
        let data = gen_text(lines);
        let bytes = data.len() as u64;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{lines}L/{:.0}KB", bytes as f64 / 1024.0)),
            &data,
            |b, data| {
                b.iter(|| LineIndex::build(black_box(data)));
            },
        );
    }
    group.finish();
}

fn bench_line_index_byte_to_line(c: &mut Criterion) {
    let mut group = c.benchmark_group("LineIndex/byte_to_line");

    for lines in [1_000, 100_000, 1_300_000] {
        let data = gen_text(lines);
        let idx = LineIndex::build(&data);
        let mid_byte = data.len() / 2;
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{lines}L")),
            &(idx, mid_byte),
            |b, (idx, mid)| {
                b.iter(|| idx.byte_to_line(black_box(*mid)));
            },
        );
    }
    group.finish();
}

fn bench_line_index_line_range_offsets(c: &mut Criterion) {
    let mut group = c.benchmark_group("LineIndex/line_range_offsets");
    let data = gen_text(1_300_000);
    let idx = LineIndex::build(&data);
    let total = idx.count();
    let first = total / 2;
    let last = (first + 60).min(total);

    group.bench_function("60-line viewport at mid", |b| {
        b.iter(|| idx.line_range_offsets(black_box(first), black_box(last)));
    });
    group.finish();
}

// ── PieceTable benchmarks ────────────────────────────────────────────────────

fn bench_piece_table_from_bytes(c: &mut Criterion) {
    let mut group = c.benchmark_group("PieceTable/from_bytes");

    for lines in [1_000, 10_000, 100_000] {
        let data = gen_text(lines);
        let bytes = data.len() as u64;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{lines}L")),
            &data,
            |b, data| {
                b.iter(|| PieceTable::from_bytes(black_box(data.clone())).unwrap());
            },
        );
    }
    group.finish();
}

fn bench_piece_table_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("PieceTable/insert");

    for lines in [1_000, 10_000] {
        let data = gen_text(lines);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{lines}L")),
            &data,
            |b, data| {
                b.iter_batched(
                    || PieceTable::from_bytes(data.clone()).unwrap(),
                    |mut pt| {
                        let mid = pt.byte_len() / 2;
                        pt.insert(black_box(mid), black_box("hello\n"));
                        pt
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_piece_table_delete(c: &mut Criterion) {
    let mut group = c.benchmark_group("PieceTable/delete");

    for lines in [1_000, 10_000] {
        let data = gen_text(lines);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{lines}L")),
            &data,
            |b, data| {
                b.iter_batched(
                    || PieceTable::from_bytes(data.clone()).unwrap(),
                    |mut pt| {
                        let mid = pt.byte_len() / 2;
                        pt.delete(black_box(mid..mid + 10));
                        pt
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_piece_table_bytes_in_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("PieceTable/bytes_in_range");

    for lines in [1_000, 100_000] {
        let data = gen_text(lines);
        let pt = PieceTable::from_bytes(data).unwrap();
        let total = pt.byte_len();
        let viewport_bytes = 60 * 82;
        let start = total / 2;
        let end = (start + viewport_bytes).min(total);

        group.throughput(Throughput::Bytes((end - start) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{lines}L")),
            &(pt, start, end),
            |b, (pt, s, e)| {
                b.iter(|| pt.bytes_in_range(black_box(*s..*e)));
            },
        );
    }
    group.finish();
}

fn bench_piece_table_line_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("PieceTable/line_count");
    let data = gen_text(100_000);
    let pt = PieceTable::from_bytes(data).unwrap();

    group.bench_function("100kL static", |b| {
        b.iter(|| black_box(pt.line_count()));
    });
    group.finish();
}

fn bench_piece_table_byte_to_line(c: &mut Criterion) {
    let mut group = c.benchmark_group("PieceTable/byte_to_line");
    let data = gen_text(100_000);
    let pt = PieceTable::from_bytes(data).unwrap();
    let mid = pt.byte_len() / 2;

    group.bench_function("100kL mid", |b| {
        b.iter(|| pt.byte_to_line(black_box(mid)));
    });
    group.finish();
}

// ── Registration ─────────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_line_index_build,
    bench_line_index_byte_to_line,
    bench_line_index_line_range_offsets,
    bench_piece_table_from_bytes,
    bench_piece_table_insert,
    bench_piece_table_delete,
    bench_piece_table_bytes_in_range,
    bench_piece_table_line_count,
    bench_piece_table_byte_to_line,
);
criterion_main!(benches);
