# CLAUDE.md — text-glyph-renderer-bench

Monorepo comparativo de motores de render de texto de alto rendimiento
(macOS / Apple Silicon). Objetivo: medir el techo de performance de cada stack
para render masivo de texto (archivo de 100 MB, 120 Hz, presupuesto de 8.3 ms
por frame). Ver `docs/architecture.md` para el diseño completo.

## Estándar nivel mundial

Barra contra la que se **construye** este repo, no contra la que se lo mide al final.
Sale del corpus de referencias del proyecto (destilados de OSS top verificados contra su
fuente primaria); cada regla cita el saqueo del que viene. Nada acá es criterio inventado.

Arquetipo: monorepo de benchmark comparativo multi-stack (harness de orquestación + PoCs de
render de texto). Áreas del corpus que aplican: Rust, text-rendering, Node/TS, Python,
creative, más el piso transversal de arquitectura.

### Piso de Craft (a-j), no negociable

Piso transversal del corpus (regla raíz: **Intención Clara / zero-guessing**). Un senior
entiende el *porqué* sin ejecutar el código.

- **a. El nombre revela la intención de dominio, no el mecanismo.** `atlas`, `shaper`,
  `line_index`, no `manager`/`handler`/`data`.
- **b. Los comentarios explican POR QUÉ, nunca QUÉ.** El comentario que parafrasea el código es
  señal de renombrar o extraer.
- **c. Superficie pública autodocumentada.** La firma comunica el contrato: si hay que leer el
  cuerpo para saber qué mide una función, la firma está mal.
- **d. Impacto mínimo al cambiar el core.** Agregar un PoC o cambiar una métrica no puede obligar
  a editar N sitios acoplados. Lógica duplicada que puede divergir (el bench compartido copiado
  en dos crates, el host de Electron copiado en cuatro PoCs) es violación directa.
- **e. Features borrables sin cirugía.** Sacar un PoC deja el harness intacto.
- **f. Flujo de datos inmutable y rastreable.** Los reportes se derivan de la corrida, no se
  editan a mano: un `*_stats.json` retocado a mano es estado mutado a escondidas.
- **g. Consistencia ante excepción.** Si un PoC falla a mitad, no queda un reporte parcial que el
  agregador lea como bueno.
- **h. Los boundaries comunican lo que pasa.** El orquestador reporta por PoC qué corrió, qué se
  salteó y por qué; un fallo que se traga sin log es violación.
- **i. Límites explícitos: timeouts y reintentos acotados.** Todo subproceso de PoC corre con
  timeout; nada espera para siempre.
- **j. Fail-closed donde importa.** Ante duda, denegar: validación de reporte ausente o
  dependencia de validación faltante es **error**, nunca un warning que deja pasar el número.

### Legibilidad en frío (k-m)

Que el artefacto demuestre qué es en 30 segundos, sin abrir el código. El modo de falla a atacar
no es inflar: es quedar **sub-descripto** (buen trabajo que no se deja ver).

- **k. El README lidera con prueba visible y framing claro.** Para este repo la prueba visible es
  la **tabla comparativa con números medidos de verdad**, en el primer screenful. Los PoCs con
  salida gráfica suman captura o GIF: en un artefacto visual, la descripción textual no cuenta
  como prueba. Un bench cuyo README no muestra su tabla está sub-descripto.
- **l. Donde prometés performance, la prueba está y es reproducible.** Números head to head entre
  los stacks nombrados, con runner y entorno declarados (hardware, versiones de toolchain, fuente
  y corpus con su hash, dispersión). Es la regla load-bearing del repo: es el producto entero, no
  un adorno. (ref: benchsuite de ripgrep, TechEmpower)
- **m. Framing honesto: el límite del claim va al lado del número.** Decir dónde NO aplica
  (`gpu_available: false`, 3B mide construcción de escena y no frames presentados, los runners
  hosteados no dan resultados publicables). La vulnerabilidad calibrada da más confianza que un
  número pulido; el cherry-pick la destruye. (ref: ripgrep, "not universally faster")

### Techo de Craft (lo que mueve de correcto a referencia)

Idea rectora: los repos de referencia convierten la calidad en **hecho machine-enforced**, no en
convención.

- **Nombres/superficie:** imposible de malusar, no solo legible. Acá el boundary único y bien
  especificado es el contrato de reporte (`shared/metrics/frame_stats.schema.json`): un reporte
  que no lo cumple no entra a la tabla. (ref: LLVM, type-state builder)
- **Encapsulamiento:** el boundary es un hecho de CI, no una convención. Una sola implementación
  del gate de validación, consumida por el runner y por los tests. (ref: import-boss de Kubernetes)
- **Integridad de estado:** invariante observable y ejercitado. `assert` significa PRUEBA
  verificada en tests, no creencia; los invariantes de `text-buffer` (equivalencia del
  mantenimiento incremental contra un rebuild completo) son el ejemplo a imitar en el resto.
  (ref: SQLite, PostgreSQL)
- **Observabilidad:** el contexto se captura en el ORIGEN (qué corpus, qué fuente, qué commit, qué
  adaptador), no se reconstruye después. (ref: tracing-error)
- **Resiliencia:** presupuesto de tiempo explícito por corrida y rama por defecto que DENIEGA. Un
  validador que se saltea porque falta una dependencia es fail-open y es el antipatrón exacto.
  (ref: AWS Builders' Library, RBAC de Kubernetes)

### Reglas enforzables del stack

**Rust** (saqueos: `rerun-io/rerun_template`, `linebender/parley`, `pop-os/cosmic-text`,
`alacritty/alacritty`):

- El set de lints vive en `[workspace.lints]` y cada crate lo hereda con `[lints] workspace = true`
  (lint set v7 de linebender, rerun_template). Nada de lints redeclarados por crate.
- El manifest declara `warn`, **CI los vuelve error** con `RUSTFLAGS: -D warnings`
  (rerun_template `rust.yml`). Ya activo en `.github/workflows/ci.yml`.
- `clippy.toml` con `msrv` en sync con `[workspace.package] rust-version` (parley usa comentarios
  "keep in sync" como contrato entre Cargo.toml, CI y README), `allow-unwrap-in-tests` y
  prohibiciones nombradas (`disallowed-macros`).
- **MSRV es invariante, no aspiración:** CI buildea con el MSRV exacto leído del manifest
  (`CONTRIBUTING.md` de alacritty: "must always build with the MSRV").
- Al limpiar una categoría entera de lint, subirla a `deny` para que no vuelva a entrar
  (cosmic-text: `#![deny(clippy::unwrap_used)]`, `#![deny(missing_debug_implementations)]`).
- `cargo fmt --all --check` y `cargo clippy --all-targets` son gates, no sugerencias (alacritty).
- Benches con `criterion` y `harness = false`, casos **nombrados por dimensión del problema**
  (ASCII fast path, mixto, stress) y pares `*_cold` / `*_warm`, con `black_box` para que el
  optimizador no los elida (cosmic-text).
- El bench de un crate vive **en ese crate**. Copiarlo en los consumidores es la violación (d) que
  este repo ya pagó una vez.
- Medir no debe costar: guard RAII que mide en `Drop` más media móvil O(1) (alacritty `meter.rs`).
- Hot path de render: cero alocación por frame, una instancia por celda con `#[repr(C)]` y un draw
  call por batch, cambio de textura solo si cambió, y el layout del vertex buffer se **mide**, no
  se asume (alacritty `glsl3.rs`, más la guía de atlas de glifos en GPU del corpus).
- Nunca `f32` crudo como clave de hash/eq: `f32::to_bits()` (cosmic-text).

**Harness Python** (saqueo: consenso verificado de httpx, FastAPI, polars, pydantic):

- Ruff con **allowlist explícita de familias**, nunca `ALL` ni defaults implícitos: `ruff.toml`
  con `E, F, I, B` (núcleo común del tier). El largo de línea lo maneja el formatter.
- Cada ignore es per-regla y lleva su POR QUÉ escrito en el config; no se apagan familias enteras.
- CI que gatea de verdad: **cero `continue-on-error`, cero `|| true`**, verificado línea a línea.
  Si hace falta reportar sin frenar, va el patrón reporte-luego-falla, no el escape.
- Fail-closed: el gate de validación de reportes rompe el build. Una sola implementación,
  importada por todos sus consumidores.
- Exit code real: el orquestador sale distinto de cero si un PoC pedido no produjo su fila.
- Si se declara un umbral (coverage, pass rate), CI lo ejecuta **sin escape** (httpx clava
  `--fail-under` en un script bajo `sh -e`, con step propio "Enforce coverage").

**PoCs Web (Node / Electron)** (saqueos: consenso n8n/Backstage/Directus, `xtermjs/xterm.js`,
`mrdoob/three.js`):

- Lint con `--max-warnings 0`: un warning rompe el build (xterm.js).
- Acciones de terceros **pineadas a SHA completo**, nunca a tag ni rama mutable (n8n, repo-wide).
- Lockfile commiteado y `npm ci` en CI; auditoría de deps de producción como gate.
- La lógica pura (packing del atlas, LRU, cálculo de líneas visibles del virtualizer) se testea
  **sin GPU** con `node --test`. Que el render pida GPU no exime al módulo puro de tener tests.
- `.editorconfig` es la fuente de verdad de formato entre editores (three.js).

**Contrato de medición** (transversal, es el corazón del repo; saqueos: `EleutherAI/lm-evaluation-harness`,
`stanford-crfm/helm`, `promptfoo`, `SWE-bench`):

- **BLOCKER.** Un número sin (a) config versionada que lo define y (b) artefacto serializado de la
  corrida que un tercero pueda re-ejecutar y auditar, **no existe**. lm-evaluation-harness usa el
  commit del codebase como unidad de replicación pareada; HELM serializa cada request y response.
- **BLOCKER.** El benchmark corre como código que **falla el build** bajo umbral, no como notebook
  ni como corrida manual que CI nunca re-ejecuta (promptfoo: exit distinto de cero bajo umbral).
- Un promedio sin n, sin percentiles ni dispersión y sin corrida reproducible es **teatro de
  métricas**.
- El oráculo se audita a sí mismo: un ground truth que nadie cuestionó es deuda, no barra (el
  propio audit de SWE-bench Verified encontró más del 59% de tareas falladas con tests defectuosos).
- Se declara la semántica de cada medición y **no se rankea entre semánticas distintas**: encode
  en CPU, render GPU end to end y construcción de escena no comparten columna.

### Documentación

- El README **nombra al proyecto igual que el manifest**: `text-glyph-renderer-bench`. Nada de
  alias ni de un nombre lindo que no matchee con el repo, el `Cargo.toml` y la URL.
- **No linkea a archivos que no existen.** Todo link relativo resuelve a un archivo trackeado
  (gate barato: chequeo de links en CI, estilo `links.yml` de rerun_template).
- **No es un molde reciclado.** Cada sección describe ESTE repo: sin secciones de plantilla
  vacías, sin "Contributing" genérico que no refleje el protocolo real, sin badges de otro
  proyecto.
- Los claims load-bearing del README y de los docstrings son **verdad verificable**. Si el README
  dice "hard failure, not a silent pass", el código falla duro; si un docstring dice "exactly one
  implementation", hay exactamente una. Prosa excelente que deja el modelo mental equivocado es
  peor que prosa ausente.
- Documento que promete un mecanismo inexistente (una sonda que ningún PoC expone, una tabla de
  predicciones que se lee como medición) se borra o se marca explícito como no implementado.
- El "por qué" vive en un registro de decisiones con fecha, motivo y alternativa descartada,
  incluidos los hallazgos NO resueltos. Es señal de alcance deliberado y se mantiene.

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
- Los thresholds y las prohibiciones nombradas de clippy viven en `clippy.toml`
  (`msrv` en sync con `[workspace.package] rust-version`). El lint del harness
  Python vive en `ruff.toml` con allowlist explícita (`E, F, I, B`); CI corre
  `ruff check shared/`.

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
