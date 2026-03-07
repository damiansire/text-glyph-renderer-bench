# text-engine-poc

Monorepo comparativo de motores de texto de alto rendimiento en macOS / Apple Silicon.

## Objetivo

Medir empíricamente el techo de rendimiento de diferentes stacks para renderizado de texto masivo (archivo de 100 MB, 120 Hz, frame budget 8.3 ms).

## Arquitectura

Ver [`docs/architecture.md`](docs/architecture.md) para el diseño completo.

## PoCs incluidos

| Carpeta | Stack | Categoría |
|---------|-------|-----------|
| `poc-1a-web-dom` | Electron + DOM | Web Sandboxed |
| `poc-1b-canvas2d` | Canvas 2D + OffscreenCanvas | Web Sandboxed |
| `poc-1c-webgpu-atlas` | WebGPU + Texture Atlas | Web Sandboxed |
| `poc-1d-webgpu-msdf` | WebGPU + MSDF | Web Sandboxed |
| `poc-2a-textkit2` | TextKit 2 (NSTextView) | Native macOS |
| `poc-2b-metal3-coretext` | Metal 3 + CoreText + Arg Buffers | Native macOS |
| `poc-3a-rust-wgpu` | Rust + wgpu + HarfBuzz | Systems |
| `poc-3b-rust-vello` | Rust + Vello | Systems |

## Setup inicial

### 1. Generar el archivo de test (100 MB, determinista)

```bash
cd shared/test-data
python3 generate_testfile.py
# → genera test_100mb.txt (~100 MB, seed=42)
```

### 2. Build Rust workspace

```bash
# Desde la raíz del monorepo
cargo build --release -p poc-3a-rust-wgpu
cargo build --release -p poc-3b-rust-vello
```

### 3. PoCs Web (Node.js / Electron)

```bash
# PoC 1A — Web DOM
cd poc-1a-web-dom && npm install && npm start

# Ejecutar benchmark de scroll sintético:
npm run benchmark
```

## Requisitos de sistema

- macOS 14+ (Sonoma) · Apple Silicon (M1 o superior)
- Xcode 15+ (para PoCs 2A, 2B)
- Rust 1.78+ (`rustup update`)
- Node.js 20+ · npm 10+
- Python 3.11+

## Estructura de métricas

Cada PoC exporta un archivo `results/<poc-id>_stats.json` con el schema definido en `shared/metrics/frame_stats.schema.json`.
