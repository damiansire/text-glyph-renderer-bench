//! range_alloc_bench.rs — Medición Fase 2 (gate "medir antes de optimizar").
//!
//! Compara las dos rutas de lectura del `TextBuffer` sobre un buffer realista
//! (~100 MB), SIN tocar el código de producción:
//!
//!   H1 — viewport (60 líneas):
//!     `bytes_in_range` (aloca un `Vec` por llamada, F5) vs
//!     `slice_pieces` (zero-alloc, ruta bendecida por el trait).
//!     Pregunta: ¿cuánto cuesta la asignación por frame y si importa frente al
//!     presupuesto de 8.3 ms.
//!
//!   H2 — materialización total (F6 / G-2):
//!     `bytes_in_range(0..byte_len())` (copia ~100 MB, la ruta del setup del 3B)
//!     vs iterar todo el documento con `slice_pieces` (zero-copy).
//!     Pregunta: costo de materializar el archivo entero vs no copiarlo.
//!
//! Las funciones de producción NO se modifican: este bench solo las invoca.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use poc_3a_rust_wgpu::{PieceTable, TextBuffer};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Genera texto sintético tipo código fuente: `lines` × ~82 bytes/línea.
/// 1.3M líneas ≈ 100 MB (el tamaño objetivo declarado en el CLAUDE.md).
fn gen_text(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * 82);
    for i in 0..lines {
        let line = format!("fn function_{i:06}() {{ /* body */ return {i}; }}\n");
        out.extend_from_slice(line.as_bytes());
    }
    out
}

// ── H1: viewport read — bytes_in_range (alloc) vs slice_pieces (zero-alloc) ───

fn bench_viewport_alloc_vs_zero(c: &mut Criterion) {
    let mut group = c.benchmark_group("H1_viewport_read");

    // Buffer de ~100 MB (1 sola pieza: el caso real de scroll read-only).
    let data = gen_text(1_300_000);
    let pt = PieceTable::from_bytes(data);
    let total = pt.byte_len();

    // Viewport de 60 líneas (~4920 bytes) a mitad de scroll.
    let viewport_bytes = 60 * 82;
    let start = total / 2;
    let end = (start + viewport_bytes).min(total);
    let span = (end - start) as u64;

    group.throughput(Throughput::Bytes(span));

    // Ruta A: aloca un Vec por llamada (F5) — el default ergonómico.
    group.bench_function(BenchmarkId::new("bytes_in_range_alloc", "60L"), |b| {
        b.iter(|| {
            let v = pt.bytes_in_range(black_box(start..end));
            black_box(v.len())
        });
    });

    // Ruta B: zero-alloc — consume las piezas in situ (suma bytes para no
    // optimizar la closure a un no-op).
    group.bench_function(BenchmarkId::new("slice_pieces_zero_alloc", "60L"), |b| {
        b.iter(|| {
            let mut acc = 0usize;
            pt.slice_pieces(black_box(start..end), |slice, _| {
                acc = acc.wrapping_add(slice.iter().fold(0usize, |a, &x| a.wrapping_add(x as usize)));
                true
            });
            black_box(acc)
        });
    });

    // Ruta B': zero-alloc puro, sin tocar los bytes (solo el costo de recorrer
    // las piezas y resolver slices) — cota inferior del camino zero-alloc.
    group.bench_function(BenchmarkId::new("slice_pieces_len_only", "60L"), |b| {
        b.iter(|| {
            let mut acc = 0usize;
            pt.slice_pieces(black_box(start..end), |slice, _| {
                acc = acc.wrapping_add(slice.len());
                true
            });
            black_box(acc)
        });
    });

    // Ruta C: COPIA con buffer reutilizado (amortiza la asignación entre frames,
    // tal como propone F5 con `bytes_in_range_into`). Mismo memcpy vectorizado
    // que `bytes_in_range`, pero SIN alocar por llamada. Aísla el costo puro de
    // la asignación = (ruta A) - (ruta C).
    group.bench_function(BenchmarkId::new("reused_buffer_copy", "60L"), |b| {
        let mut scratch: Vec<u8> = Vec::with_capacity(viewport_bytes);
        b.iter(|| {
            scratch.clear();
            pt.slice_pieces(black_box(start..end), |slice, _| {
                scratch.extend_from_slice(slice);
                true
            });
            black_box(scratch.len())
        });
    });

    group.finish();
}

// ── H2: full materialization (F6) — copiar ~100 MB vs iterar zero-copy ────────

fn bench_full_materialize_vs_iterate(c: &mut Criterion) {
    let mut group = c.benchmark_group("H2_full_document");
    // Pocas muestras: cada iteración copia ~100 MB y es lenta.
    group.sample_size(10);

    let data = gen_text(1_300_000);
    let pt = PieceTable::from_bytes(data);
    let total = pt.byte_len();
    group.throughput(Throughput::Bytes(total as u64));

    // Ruta del setup del 3B (F6): materializa el archivo completo a un Vec.
    group.bench_function(BenchmarkId::new("bytes_in_range_full", "100MB"), |b| {
        b.iter(|| {
            let v = pt.bytes_in_range(black_box(0..total));
            black_box(v.len())
        });
    });

    // Alternativa zero-copy: recorrer todo el documento con slice_pieces,
    // sumando los bytes (toca toda la memoria, igual que la copia, pero sin
    // alocar 100 MB nuevos).
    group.bench_function(BenchmarkId::new("slice_pieces_full_sum", "100MB"), |b| {
        b.iter(|| {
            let mut acc = 0u64;
            pt.slice_pieces(black_box(0..total), |slice, _| {
                for &x in slice {
                    acc = acc.wrapping_add(x as u64);
                }
                true
            });
            black_box(acc)
        });
    });

    group.finish();
}

criterion_group!(measure, bench_viewport_alloc_vs_zero, bench_full_materialize_vs_iterate);
criterion_main!(measure);
