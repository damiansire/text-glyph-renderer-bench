# Plan: atlas de glifos paginado (multi-textura) — PoC 2B (P9)

> **Estado: NO IMPLEMENTADO / NO VERIFICADO.** Swift/Metal no compila en el
> entorno de auditoría (Windows), así que este documento es un **plan de
> implementación**, no código probado. Cualquier andamiaje incluido en
> `TextureAtlasSwift.swift` está marcado como tal y debe validarse con un
> toolchain Metal (macOS + Xcode) antes de darse por bueno.

## Problema (auditoría P9)

`TextureAtlasSwift.allocate(w:h:)` ante atlas lleno hace:

```swift
resetCount += 1
shelves = []; nextShelfY = 0; cache = [:]
return allocate(w: w, h: h)
```

Es decir: cuando la única textura de 2048×2048 se llena, **tira todo el
cache** y vuelve a rasterizar desde cero. En un scroll largo con muchos glifos
distintos (CJK, emoji, símbolos, varios tamaños/fuentes), cada overflow fuerza
re-rasterización masiva **justo dentro del path medido** de 8.3 ms/frame. El
propio comentario lo admite ("simplified; production would use free-list").

La primera vuelta de fixes dejó un contador `resetCount` para que el harness
**detecte y descarte** runs contaminados por un reset, pero eso es mitigación
de medición, no la solución. La solución es **paginar**: al llenarse una
textura, abrir una **nueva** y seguir, sin tocar lo ya cacheado.

## Diseño objetivo: atlas paginado

### Estructura de datos

```text
Atlas {
  pages: [AtlasPage]        // cada una es una MTLTexture 2048×2048 r8Unorm
  activePage: Int           // índice de la página donde se intenta empaquetar
  cache: [UInt64: GlyphAtlasSlot]   // sin cambios de clave; el slot gana `page`
}

AtlasPage {
  texture: MTLTexture
  shelves: [(nextX, y, height)]
  nextShelfY: Int
}
```

`GlyphAtlasSlot` gana un campo:

```swift
var page: Int   // qué textura del array contiene este glifo
```

### Algoritmo de `allocate`

1. Intentar empaquetar en `pages[activePage]` con el mismo shelf-packing actual.
2. Si no entra en ninguna shelf existente y `nextShelfY + h > size`:
   - **NO** limpiar nada. En su lugar:
   - Si `activePage` es la última página, **crear una página nueva**
     (`makePage()`), apilarla en `pages`, y `activePage = pages.count - 1`.
   - Reintentar el empaquetado en la nueva página (recursión acotada: a lo sumo
     una página nueva por glifo, y el glifo cabe porque `w,h <= size`).
3. Devolver `(page: activePage, atlasX, atlasY)`.

Quitar por completo la rama `shelves = []; cache = [:]`. `resetCount` deja de
incrementarse en operación normal (se puede conservar como métrica histórica o
eliminar; recomiendo **conservarlo en 0** como invariante verificable: "un run
sano nunca resetea").

### Tope de páginas (presión de memoria)

Cada página son 2048·2048·1 byte = **4 MB**. Hay que acotar el crecimiento:

- Definir `maxPages` (p. ej. 8 → 32 MB de atlas, más que suficiente para el
  corpus del bench). Exponerlo en el init.
- Al alcanzar `maxPages` y necesitar otra, recién ahí aplicar una política de
  evicción **por página** (no global): elegir la página con menor `hitCount`
  reciente (aging generacional), invalidar **solo las entradas de cache que
  apuntan a esa página**, y reusarla. Esto preserva la mayoría del cache en vez
  de tirarlo entero. Para el bench, `maxPages` debería elegirse de modo que el
  reset **no ocurra** durante los 30 s de scroll; si ocurre, el run se descarta
  igual que hoy vía contador.

## Cambios en el renderer (lo que hace el fix "no trivial")

Hoy `MetalRenderer` asume **una sola** textura de atlas:

- `MetalRenderer.swift:137` crea su propio `atlasTexture` y lo cablea en el
  argument buffer bindless (`argumentEncoder.setTexture(atlasTexture, index: 0)`,
  línea 151), y lo marca residente con `useResource(atlasTexture, …)` (línea 255).
- El vertex emit (líneas 214-217) calcula UVs con un único `atlasInv` y **no**
  lleva índice de textura por vértice.

Para soportar N páginas hay dos caminos:

**Opción A — array de texturas bindless (recomendada, Metal 3 / Tier 2).**
- Cambiar el argument buffer para contener un **array** de texturas
  (`setTextures(_:range:)`) en vez de una sola en index 0.
- Agregar al layout de vértice un `texIndex` (UInt32 o Float empaquetado) por
  vértice, escrito en `emit(...)` a partir de `slot.page`.
- En el fragment shader, indexar `textures[texIndex]` para muestrear. Requiere
  `[[texture_array]]` o un argument-buffer array y `metalArgumentBuffersTier2`.
- `useResource` debe marcar residentes **todas** las páginas vivas.
- Sincronizar `MTLTextureDescriptor` / `replace(region:)`: hoy
  `TextureAtlasSwift` mantiene su **propia** `texture` (línea 21) mientras
  `MetalRenderer` crea **otra** `atlasTexture` (137). Esa duplicación ya es
  sospechosa; al paginar hay que unificar: las páginas deben ser las texturas
  que el renderer realmente bindea y a las que el rasterizador escribe. Esto es
  parte de por qué el fix "cruza" módulos.

**Opción B — partición por draw-call (más simple, peor).**
- Agrupar los glifos por `slot.page`, emitir un `drawPrimitives` por página,
  rebindando la textura activa entre draws. Evita el shader bindless pero rompe
  el batch único y suma overhead de encoder por página; sólo aceptable si la
  Opción A no es viable por tier de hardware.

## Plan de tests (cuando haya toolchain Metal)

1. **Sin reset bajo carga.** Generar > 2048²/glyph_area glifos distintos y
   afirmar `atlas.resetCount == 0` y `atlas.pageCount > 1`.
2. **Cache estable a través del overflow.** Rasterizar un glifo G, forzar
   overflow llenando con otros, volver a pedir G → debe ser **hit** (su slot,
   incluido `page`, sigue válido), no miss.
3. **UV correctos por página.** Un glifo en `page > 0` muestrea de la textura
   correcta (test de render diferencial: comparar el quad contra la
   rasterización CoreGraphics de referencia).
4. **Tope de memoria.** Con `maxPages` bajo, al excederlo la evicción invalida
   **solo** las entradas de la página reusada (las de otras páginas siguen hit).

## Andamiaje seguro incluido ahora (NO crítico, NO verificado)

En `TextureAtlasSwift.swift` se agrega únicamente superficie **observabilidad +
TODO**, sin tocar el path de rasterización ni el renderer:

- `pageCount` (hoy siempre `1`) para que el harness pueda empezar a registrar
  cuántas páginas usó un run; al paginar de verdad, este getter pasa a
  `pages.count`.
- Un `// TODO(P9)` puntual en `allocate` que enlaza a este documento.

Estos son cambios aditivos de Swift que **no se pudieron compilar en este
entorno**; revisar al portar a macOS.
