# CLAUDE.md — text-glyph-renderer-bench

Monorepo comparativo de motores de render de texto de alto rendimiento
(macOS / Apple Silicon). Objetivo: medir el techo de performance de cada stack
para render masivo de texto (archivo de 100 MB, 120 Hz, presupuesto de 8.3 ms
por frame). Ver `docs/architecture.md` para el diseño completo.

## Workspace

- Crates Rust: `poc-3a-rust-wgpu` (wgpu + rustybuzz), `poc-3b-rust-vello`
  (Vello, vector GPU). Resto de PoCs son Web (Node/Electron) y nativos macOS.
- MSRV: **fuente única de verdad** en `[workspace.package] rust-version` del
  `Cargo.toml` raíz. Mantener en sync con el README al subirla.

## Lints (gate de calidad)

El set vive en `[workspace.lints]` del `Cargo.toml` raíz y cada crate lo hereda
con `[lints] workspace = true`. Hoy todo está en `"warn"` a propósito: el código
usa `unsafe` (memmap) y `unwrap` en hot paths de PoC, así que los lints
documentan deuda sin romper el build.

- `clippy::cast_possible_truncation` es el de mayor ROI: en el render path se
  castea f32 de subpixel a i32 de pixel y ahí se esconden off-by-one silenciosos.
- Al limpiar una categoría (p. ej. eliminar todos los `unwrap` de código de
  librería), subir ese lint a `"deny"` para que no vuelva a entrar.

## Protocolo de performance

- Si tocás el hot path (shaping, rasterización, layout del vertex buffer, atlas),
  traé números antes/después. No optimices a ciegas.
- Medí siempre sobre Rust **stable** (no nightly) para bajar la varianza.
- Benches con `criterion` (`harness = false`). La métrica honesta de un glyph
  renderer separa **cold cache** (parse de font + shape + cache miss) de **warm
  cache** (glifo ya cacheado). Registrá los casos como pares `*_cold` / `*_warm`;
  medir solo warm sobre-vende el rendimiento de arranque.
- Medir no debe costar: si agregás un medidor de frame en vivo, usá un guard RAII
  (se mide en `Drop`, no se puede olvidar de cerrar) con media móvil O(1).

## Convenciones de código

- Comentarios con punto final, siempre.
- Nunca usar `f32` crudo como clave de hash/eq: usar `f32::to_bits()`.
- Tipos públicos implementan `Debug` (lint `missing_debug_implementations`).

## Atribución

Las contribuciones no deben incluir atribución a LLMs ni `Co-Authored-By`. Si se
saquean patrones de otros proyectos, se reescriben acá; no se pega código ajeno.
