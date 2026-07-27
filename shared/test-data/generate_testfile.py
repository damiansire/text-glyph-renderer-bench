#!/usr/bin/env python3
"""
generate_testfile.py — Generador determinista del corpus de 100 MB para text-engine-poc.

Corpus mixto:
  - Texto en prosa en inglés y español (ASCII, acentos)
  - Caracteres Unicode variados (emojis, símbolos matemáticos, flechas)
  - Caracteres CJK (Chino, Japonés, Coreano) — trigger de font fallback
  - Texto RTL simulado (árabe/hebreo) — test de bidi shaping
  - Fragmentos de código fuente con indentación
  - Líneas largas (800+ caracteres) y líneas vacías
  - Números, fechas, URLs

Uso:
  python3 generate_testfile.py                    # → test_100mb.txt (100 MB)
  python3 generate_testfile.py --size 10          # → test_10mb.txt  (10 MB)
  python3 generate_testfile.py --verify           # verifica SHA-256 del corpus canónico

Seed: 42 (siempre produce el mismo archivo)
"""

import argparse
import hashlib
import random
import sys
import time
from pathlib import Path

# El corpus y los mensajes de progreso contienen Unicode (→, box-drawing, CJK).
# En consolas no-UTF8 (p.ej. cp1252 en Windows) un `print` de esos caracteres
# lanzaría UnicodeEncodeError. Forzamos UTF-8 en stdout/stderr para que el
# generador corra igual en cualquier plataforma. El archivo de salida ya se abre
# explícitamente con encoding="utf-8", así que su contenido no se ve afectado.
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8")
    except (AttributeError, ValueError):
        pass

# ─── Corpus de segmentos ────────────────────────────────────────────────

PROSE_EN = [
    "The quick brown fox jumps over the lazy dog.",
    "In the beginning was the Word, and the Word was with God.",
    "It was the best of times, it was the worst of times, it was the age of wisdom.",
    "All happy families are alike; each unhappy family is unhappy in its own way.",
    "Call me Ishmael. Some years ago—never mind how long precisely—having little money in my purse.",
    "It is a truth universally acknowledged, that a single man in possession of a good fortune, must be in want of a wife.",
    "The sky above the port was the color of television, tuned to a dead channel.",
    "Many years later, as he faced the firing squad, Colonel Aureliano Buendía was to remember that distant afternoon.",
    "You are not the kind of guy who would be at a place like this at this time of the morning.",
    "Happy families are all alike; every unhappy family is unhappy in its own way.",
]

PROSE_ES = [
    "En un lugar de la Mancha, de cuyo nombre no quiero acordarme, no ha mucho tiempo que vivía un hidalgo.",
    "Cien años de soledad fue narrada por Gabriel García Márquez con maestría sin precedentes.",
    "El tiempo es oro, pero la paciencia es platino en el arte de la programación.",
    "La arquitectura de software es el arte de tomar decisiones que son difíciles de cambiar.",
    "Todo lo que necesitas saber sobre el rendimiento es que debes medirlo antes de optimizarlo.",
    "Los algoritmos eficientes son la diferencia entre una aplicación que responde y una que frustra.",
    "En el principio creó Dios los cielos y la tierra, y la tierra estaba desordenada y vacía.",
    "Había una vez en un reino muy lejano un programador que creyó que premature optimization era la raíz de todo mal.",
    "Las estructuras de datos son el fundamento sobre el cual se construyen los algoritmos más elegantes.",
    "El renderizado de texto es sorprendentemente complejo: ligaturas, kerning, bidi, fallback... todo importa.",
]

UNICODE_MISC = [
    "Mathematical symbols: ∀x∈ℝ, ∂f/∂x = lim_{h→0} (f(x+h)-f(x))/h ≈ 3.14159265358979",
    "Arrows and operators: ← → ↑ ↓ ↔ ⇒ ⇔ ∈ ∉ ⊂ ⊃ ∪ ∩ ∅ ∞ ≠ ≤ ≥ ±",
    "Emojis: 🚀 🎯 🔥 💡 ⚡ 🎸 🌊 🦊 🐉 🌸 🍎 🗻 🌍 🎭 🏆",
    "Currency: $ € £ ¥ ₹ ₽ ₿ ₩ ₺ ฿ ₦ ₴ ₫ ₸ ₱",
    "Greek: α β γ δ ε ζ η θ ι κ λ μ ν ξ ο π ρ σ τ υ φ χ ψ ω",
    "Arrows extended: ⟵ ⟶ ⟷ ⟸ ⟹ ⟺ ↩ ↪ ↫ ↬ ↭ ↮ ↯ ↰ ↱ ↲ ↳",
    "Box drawing: ┌─┬─┐ │ │ │ ├─┼─┤ │ │ │ └─┴─┘ ╔═╦═╗ ║ ║ ║ ╠═╬═╣ ║ ║ ║ ╚═╩═╝",
    "Music: ♩ ♪ ♫ ♬ ♭ ♮ ♯ 𝄞 𝄢 𝅘𝅥𝅮 𝅗𝅥",
]

CJK_SEGMENTS = [
    "日本語のテキストレンダリングはとても複雑です。カーニングとリガチャが重要です。",
    "中文文本渲染需要处理数千个字符，字体回退机制至关重要。",
    "한국어 텍스트 렌더링은 복잡한 글꼴 처리가 필요합니다.",
    "漢字とひらがなとカタカナが混在するテキストの処理は難しい。例えば：東京、大阪、京都。",
    "计算机科学中，文本渲染是一个令人着迷的领域，涉及字体技术、排版算法和图形学。",
    "中文、日文和韩文统称为CJK字符，在Unicode标准中占有重要地位。",
    "フォントフォールバックメカニズムは、プライマリフォントにない文字を表示するために使用されます。",
    "글꼴 대체 메커니즘은 기본 글꼴에 없는 문자를 표시하는 데 사용됩니다.",
]

RTL_SEGMENTS = [
    "النص العربي يُكتب من اليمين إلى اليسار، مما يجعل تخطيط النص معقداً.",
    "שפות כתיבה מימין לשמאל כמו עברית ערבית מציבות אתגרים ייחודיים.",
    "الخوارزميات الثنائية الاتجاه (BiDi) ضرورية لعرض النص متعدد اللغات.",
    "تقنيات رسم الخط تشمل: HarfBuzz, CoreText, DirectWrite وFreeType.",
]

CODE_SNIPPETS = [
    """fn piece_table_insert(table: &mut PieceTable, at: usize, text: &str) {
    let add_start = table.add_buffer.len();
    table.add_buffer.push_str(text);
    // Split existing piece at insertion point
    let (piece_idx, offset) = table.find_piece(at);
    table.pieces.insert(piece_idx + 1, Piece {
        buffer: BufferType::Add,
        start: add_start,
        len: text.len(),
    });
}""",
    """// Metal Argument Buffer — bindless rendering
struct GlyphDrawArguments {
    device::texture2d<float> atlasTexture [[id(0)]];
    device::sampler atlasSampler          [[id(1)]];
    device const GlyphUV* glyphUVs        [[id(2)]];
    device const float4x4* transforms     [[id(3)]];
};

vertex VertexOut glyph_vertex(
    uint instanceId [[instance_id]],
    constant GlyphDrawArguments& args [[buffer(0)]]
) {
    float4x4 transform = args.transforms[instanceId];
    GlyphUV uv = args.glyphUVs[instanceId];
    // ...
}""",
    """// WebGPU MSDF Fragment Shader (WGSL)
fn median(r: f32, g: f32, b: f32) -> f32 {
    return max(min(r, g), min(max(r, g), b));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let msd: vec3<f32> = textureSample(msdf_atlas, atlas_sampler, in.uv).rgb;
    let sd: f32 = median(msd.r, msd.g, msd.b);
    let w: f32 = fwidth(sd) * 0.7;
    let alpha: f32 = smoothstep(0.5 - w, 0.5 + w, sd);
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}""",
    """import Metal
import CoreText

class TextureAtlas {
    private let texture: MTLTexture
    private var shelves: [(y: Int, nextX: Int, height: Int)] = []
    private var lruCache: [GlyphKey: AtlasSlot] = [:]
    private var lruList: DoublyLinkedList<GlyphKey> = .init()
    
    /// Insert glyph using shelf packing algorithm
    func insert(_ glyph: CGGlyph, font: CTFont, size: CGFloat) -> AtlasSlot {
        let bbox = CTFontGetBoundingRectsForGlyphs(font, .default, [glyph], nil, 1)
        let w = Int(ceil(bbox.width)) + 2   // 1px padding each side
        let h = Int(ceil(bbox.height)) + 2
        // Find shelf with enough horizontal space...
        return allocateSlot(width: w, height: h)
    }
}""",
    """use memchr::memchr_iter;
use memmap2::{Mmap, MmapOptions};
use std::fs::File;

pub struct PieceTable {
    original: Mmap,           // file mmap — zero-copy read-only
    add_buffer: Vec<u8>,      // heap — append-only writes
    pieces: Vec<Piece>,
    line_index: Vec<u32>,     // byte offsets of each \\n
}

impl PieceTable {
    pub fn from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { MmapOptions::new().populate().map(&file)? };
        let line_index = Self::build_line_index(&mmap);
        let len = mmap.len();
        Ok(Self {
            original: mmap,
            add_buffer: Vec::new(),
            pieces: vec![Piece { buffer: BufferType::Original, start: 0, len }],
            line_index,
        })
    }
    
    fn build_line_index(data: &[u8]) -> Vec<u32> {
        let mut offsets = vec![0u32]; // línea 0 empieza en byte 0
        offsets.extend(memchr_iter(b'\\n', data).map(|i| (i + 1) as u32));
        offsets
    }
}""",
]

# ─── Líneas largas y líneas vacías ──────────────────────────────────────

def make_long_line(rng: random.Random, length: int = 800) -> str:
    """Genera una línea larga de texto mixto."""
    words = ["performance", "rendering", "benchmark", "latency", "throughput",
             "memory", "shader", "texture", "atlas", "glyph", "kerning", "font",
             "unicode", "UTF-8", "pipeline", "encoder", "buffer", "vertex",
             "fragment", "compute", "mmap", "zero-copy", "UMA", "SIMD", "NEON"]
    result = []
    total = 0
    while total < length:
        word = rng.choice(words)
        result.append(word)
        total += len(word) + 1
    return " ".join(result)


def make_url_line(rng: random.Random) -> str:
    domains = ["github.com", "crates.io", "developer.apple.com", "gpuweb.github.io", "doc.rust-lang.org"]
    paths = ["/text-engine/poc", "/crates/memchr", "/metal/overview", "/wgsl", "/std/collections"]
    return f"See: https://{rng.choice(domains)}{rng.choice(paths)}?ref={rng.randint(1000,9999)}&experimental=true"


# ─── Generador principal ─────────────────────────────────────────────────

SEGMENT_POOLS = [
    (0.30, PROSE_EN),
    (0.20, PROSE_ES),
    (0.15, UNICODE_MISC),
    (0.15, CJK_SEGMENTS),
    (0.07, RTL_SEGMENTS),
    (0.13, CODE_SNIPPETS),
]

# Verificar que los pesos suman 1.0
assert abs(sum(w for w, _ in SEGMENT_POOLS) - 1.0) < 1e-9, "Weights must sum to 1.0"


def choose_pool(rng: random.Random):
    r = rng.random()
    cumulative = 0.0
    for weight, pool in SEGMENT_POOLS:
        cumulative += weight
        if r < cumulative:
            return pool
    return SEGMENT_POOLS[-1][1]


def generate_line(rng: random.Random) -> str:
    """Genera una línea de texto."""
    r = rng.random()
    if r < 0.02:
        # Línea vacía
        return ""
    elif r < 0.05:
        # Línea larga
        return make_long_line(rng, rng.randint(400, 1000))
    elif r < 0.07:
        # URL / referencia
        return make_url_line(rng)
    else:
        pool = choose_pool(rng)
        segment = rng.choice(pool)
        # Añadir un número de línea ocasional para hacer el corpus más realista
        if rng.random() < 0.15:
            segment = f"[{rng.randint(1, 99999):5d}] {segment}"
        return segment


# SHA-256 canónica del corpus por tamaño, con la seed por defecto (42).
# Verificado idéntico entre corridas, con PYTHONHASHSEED distinto y contra el
# meta generado un mes antes en otra máquina. Es el ancla externa: sin esto
# `--verify` se comparaba contra un sidecar que él mismo había escrito.
EXPECTED_SHA256: dict[int, str] = {
    1: "fa1ec26a00c2c62a2c609f2954b636d3bef16f2ce81e81faa791d69fa8889c55",
}


def sha256_of(path: Path) -> str:
    sha = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            sha.update(chunk)
    return sha.hexdigest()


def generate_testfile(output_path: Path, target_mb: int = 100, seed: int = 42) -> None:
    rng = random.Random(seed)
    target_bytes = target_mb * 1024 * 1024
    output_path = output_path.resolve()

    print(f"Generando corpus de {target_mb} MB (seed={seed})...")
    print(f"  → {output_path}")

    t0 = time.monotonic()
    written = 0
    lines_written = 0
    sha = hashlib.sha256()

    # `newline="\n"` evita que el modo texto traduzca \n→\r\n en Windows: sin
    # esto el corpus quedaba con CRLF y un SHA-256 distinto al canónico de
    # macOS/Linux (rompiendo la reproducibilidad cross-plataforma; el `--verify`
    # no lo detectaba porque compara contra un meta que él mismo generó).
    with open(output_path, "w", encoding="utf-8", newline="\n", buffering=256 * 1024) as f:
        while written < target_bytes:
            line = generate_line(rng)
            line_bytes = (line + "\n").encode("utf-8")
            f.write(line + "\n")
            sha.update(line_bytes)
            written += len(line_bytes)
            # Un registro puede ser multilínea (los CODE_SNIPPETS traen \n
            # embebidos), así que se cuentan los saltos de línea reales del
            # archivo, no los registros generados: el sidecar documenta el
            # corpus que ancla las mediciones y subcontaba ~3x.
            lines_written += line.count("\n") + 1

            if lines_written % 100_000 == 0:
                pct = min(100, 100 * written / target_bytes)
                elapsed = time.monotonic() - t0
                print(f"  {pct:5.1f}%  {written / 1e6:6.1f} MB  {lines_written:,} líneas  ({elapsed:.1f}s)", end="\r")

    elapsed = time.monotonic() - t0
    digest = sha.hexdigest()
    actual_mb = written / 1_048_576

    print(f"\n✓ Generado en {elapsed:.2f}s")
    print(f"  Tamaño:    {actual_mb:.2f} MB  ({written:,} bytes)")
    print(f"  Líneas:    {lines_written:,}")
    print(f"  SHA-256:   {digest}")
    print(f"  Archivo:   {output_path}")

    # Guardar metadatos junto al archivo
    meta_path = output_path.with_suffix(".meta.json")
    import json
    meta = {
        "seed": seed,
        "target_mb": target_mb,
        "actual_bytes": written,
        "actual_mb": round(actual_mb, 4),
        "lines": lines_written,
        "sha256": digest,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "encoding": "UTF-8",
    }
    with open(meta_path, "w") as mf:
        json.dump(meta, mf, indent=2)
    print(f"  Metadata:  {meta_path}")


def verify_file(path: Path, target_mb: int | None = None) -> None:
    # Golden fijado en el repo. Antes `--verify` comparaba contra el
    # `.meta.json` que la misma corrida acababa de escribir, o sea contra sí
    # mismo: una tautología que no podía detectar una deriva del generador.
    # Con el golden acá, cambiar el corpus rompe el gate, que es el punto de
    # declarar que la corrida es reproducible.
    if target_mb is not None and target_mb in EXPECTED_SHA256:
        expected = EXPECTED_SHA256[target_mb]
        print(f"Verificando {path.name} contra el golden del repo ({target_mb} MB)...")
        digest = sha256_of(path)
        if digest == expected:
            print(f"✓ SHA-256 correcta: {digest}")
            return
        print("✗ SHA-256 NO coincide con el golden del repo!")
        print(f"  Esperado: {expected}")
        print(f"  Obtenido: {digest}")
        sys.exit(1)

    meta_path = path.with_suffix(".meta.json")
    if not meta_path.exists():
        print(f"ERROR: No se encontró {meta_path}")
        sys.exit(1)
    import json
    with open(meta_path) as f:
        meta = json.load(f)

    print(f"Verificando SHA-256 de {path.name} contra su sidecar (sin golden "
          f"fijado para este tamaño)...")
    digest = sha256_of(path)

    if digest == meta["sha256"]:
        print(f"✓ SHA-256 correcta: {digest}")
    else:
        print("✗ SHA-256 NO coincide!")
        print(f"  Esperado: {meta['sha256']}")
        print(f"  Obtenido: {digest}")
        sys.exit(1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Genera corpus de texto de prueba determinista.")
    parser.add_argument("--size", type=int, default=100, help="Tamaño objetivo en MB (default: 100)")
    parser.add_argument("--seed", type=int, default=42, help="Semilla aleatoria (default: 42)")
    parser.add_argument("--output", type=str, default=None, help="Archivo de salida (default: test_{size}mb.txt)")
    parser.add_argument("--verify", action="store_true", help="Verificar SHA-256 del archivo existente")
    args = parser.parse_args()

    script_dir = Path(__file__).parent
    out_name = args.output or f"test_{args.size}mb.txt"
    out_path = script_dir / out_name

    if args.verify:
        verify_file(out_path, target_mb=args.size if args.seed == 42 else None)
    else:
        generate_testfile(out_path, target_mb=args.size, seed=args.seed)
