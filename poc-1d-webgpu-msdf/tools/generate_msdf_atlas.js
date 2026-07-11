/**
 * generate_msdf_atlas.js — Generador de fallback en Node.js puro para el
 * atlas de PoC 1D, para entornos sin el binario nativo `msdf-atlas-gen`
 * (ver tools/generate_msdf_atlas.sh, que sigue siendo el camino real cuando
 * ese binario está disponible: brew install msdf-atlas-gen).
 *
 * ADVERTENCIA HONESTA — esto NO es MSDF (Multi-channel Signed Distance
 * Field) real:
 *
 *   Este script calcula un campo de distancia con signo de UN SOLO CANAL
 *   (fuerza bruta: distancia mínima al segmento de borde más cercano de la
 *   silueta del glifo, con signo según adentro/afuera por regla even-odd) y
 *   escribe ese mismo valor en los tres canales R, G y B del PNG de salida.
 *
 *   `glyph_msdf.frag.wgsl` calcula `median(r,g,b)` para reconstruir la
 *   distancia — con r=g=b ese median es exactamente el valor de un solo
 *   canal, así que el shader renderiza texto con antialiasing correcto SIN
 *   necesitar cambios. Lo que NO se obtiene es el beneficio real de MSDF:
 *   preservar esquinas agudas cuando los tres canales disienten entre sí
 *   (technique de Chlumsky). Este generador produce un SDF de un canal
 *   empaquetado en la forma de un atlas MSDF, no una verdadera resolución
 *   multi-canal. Documentado también en el commit y en el reporte de tarea.
 *
 * Alcance: 95 glifos imprimibles ASCII (codepoints 32–126). Full
 * Latin/CJK está fuera de alcance (ver comentario del propio .sh sobre
 * necesitar Git LFS para eso).
 *
 * Método:
 *   1. Parsea contornos reales de glifos con opentype.js (pure JS, sin
 *      deps nativas) desde ../shared/fonts/InterVariable.ttf.
 *   2. Por glifo: aplana curvas Bézier a segmentos de línea, rasteriza en
 *      una celda de GLYPH_SIZE×GLYPH_SIZE px (48, igual al .sh) con
 *      distancia por fuerza bruta (aceptable: 95 glifos × 48×48 px es
 *      chico — no hace falta 8SSEDT).
 *   3. Empaqueta las celdas en grilla dentro de un PNG vía `pngjs` (pure
 *      JS) y emite el JSON de metadata con la forma que `_parseAtlasMeta`
 *      de src/renderer.js espera (atlas.width/distanceRange +
 *      glyphs[].{unicode,advance,atlasBounds,planeBounds}).
 *
 * Uso:
 *   cd poc-1d-webgpu-msdf
 *   npm run gen-atlas
 */

'use strict';

const fs = require('fs');
const path = require('path');
const opentype = require('opentype.js');
const { PNG } = require('pngjs');

// ── Configuración ────────────────────────────────────────────────────────
// GLYPH_SIZE y PXRANGE se mantienen iguales a tools/generate_msdf_atlas.sh
// para que las suposiciones de px_range/atlas_size del shader/uniform buffer
// sigan siendo razonables.
const GLYPH_SIZE = 48;   // px por celda del atlas
const PXRANGE = 4;       // distancia (en px del atlas) que cubre el campo
const CHARSET_START = 32;   // ' '
const CHARSET_END = 126;    // '~'
// src/renderer.js divide todo por 64 (`scale = fontSize / 64`) de forma fija
// (comentario: "MSDF was generated at 64px"). No podemos tocar renderer.js,
// así que planeBounds/advance se expresan en píxeles a esa referencia de
// 64px-por-em para que el resultado visual sea proporcional a lo que ese
// código ya asume.
const REFERENCE_EM_PX = 64;

const FONT_PATH = path.join(__dirname, '..', '..', 'shared', 'fonts', 'InterVariable.ttf');
const OUTPUT_DIR = path.join(__dirname, '..', 'assets');
const ATLAS_PNG = path.join(OUTPUT_DIR, 'inter_msdf_atlas.png');
const ATLAS_JSON = path.join(OUTPUT_DIR, 'inter_msdf_atlas.json');

// ── Geometría: aplanado de curvas + distancia a segmento ───────────────────

function flattenPathToContours(path_, steps = 10) {
    const contours = [];
    let current = null;
    let cur = { x: 0, y: 0 };
    let start = { x: 0, y: 0 };

    for (const cmd of path_.commands) {
        switch (cmd.type) {
            case 'M':
                current = [];
                contours.push(current);
                cur = { x: cmd.x, y: cmd.y };
                start = { ...cur };
                current.push({ ...cur });
                break;
            case 'L':
                cur = { x: cmd.x, y: cmd.y };
                current.push({ ...cur });
                break;
            case 'C':
                for (let i = 1; i <= steps; i++) {
                    const t = i / steps;
                    const mt = 1 - t;
                    const x = mt * mt * mt * cur.x + 3 * mt * mt * t * cmd.x1 + 3 * mt * t * t * cmd.x2 + t * t * t * cmd.x;
                    const y = mt * mt * mt * cur.y + 3 * mt * mt * t * cmd.y1 + 3 * mt * t * t * cmd.y2 + t * t * t * cmd.y;
                    current.push({ x, y });
                }
                cur = { x: cmd.x, y: cmd.y };
                break;
            case 'Q':
                for (let i = 1; i <= steps; i++) {
                    const t = i / steps;
                    const mt = 1 - t;
                    const x = mt * mt * cur.x + 2 * mt * t * cmd.x1 + t * t * cmd.x;
                    const y = mt * mt * cur.y + 2 * mt * t * cmd.y1 + t * t * cmd.y;
                    current.push({ x, y });
                }
                cur = { x: cmd.x, y: cmd.y };
                break;
            case 'Z':
                if (current && (cur.x !== start.x || cur.y !== start.y)) {
                    current.push({ ...start });
                }
                cur = { ...start };
                break;
            default:
                break;
        }
    }
    return contours.filter(c => c.length >= 2);
}

function contoursToEdges(contours) {
    const edges = [];
    for (const c of contours) {
        for (let i = 0; i < c.length; i++) {
            const a = c[i];
            const b = c[(i + 1) % c.length];
            if (a.x === b.x && a.y === b.y) continue;
            edges.push([a.x, a.y, b.x, b.y]);
        }
    }
    return edges;
}

function distToSegment(px, py, x1, y1, x2, y2) {
    const dx = x2 - x1, dy = y2 - y1;
    const lenSq = dx * dx + dy * dy;
    let t = lenSq === 0 ? 0 : ((px - x1) * dx + (py - y1) * dy) / lenSq;
    t = Math.max(0, Math.min(1, t));
    const cx = x1 + t * dx, cy = y1 + t * dy;
    const ddx = px - cx, ddy = py - cy;
    return Math.sqrt(ddx * ddx + ddy * ddy);
}

// Regla even-odd por ray casting horizontal.
function pointInPolygon(px, py, edges) {
    let inside = false;
    for (const [x1, y1, x2, y2] of edges) {
        const crosses = (y1 > py) !== (y2 > py);
        if (!crosses) continue;
        const xIntersect = x1 + (py - y1) * (x2 - x1) / (y2 - y1);
        if (px < xIntersect) inside = !inside;
    }
    return inside;
}

function rasterizeGlyphSDF(edges, cellSize, pxrange) {
    const cell = new Float32Array(cellSize * cellSize); // signed distance en px, positivo = adentro
    if (edges.length === 0) {
        cell.fill(-pxrange); // celda vacía (p.ej. espacio): totalmente "afuera"
        return cell;
    }
    for (let y = 0; y < cellSize; y++) {
        const cy = y + 0.5;
        for (let x = 0; x < cellSize; x++) {
            const cx = x + 0.5;
            let minDist = Infinity;
            for (const [x1, y1, x2, y2] of edges) {
                const d = distToSegment(cx, cy, x1, y1, x2, y2);
                if (d < minDist) minDist = d;
            }
            const inside = pointInPolygon(cx, cy, edges);
            cell[y * cellSize + x] = inside ? minDist : -minDist;
        }
    }
    return cell;
}

// ── Main ─────────────────────────────────────────────────────────────────

function main() {
    if (!fs.existsSync(FONT_PATH)) {
        console.error(`No se encontró la fuente: ${FONT_PATH}`);
        process.exit(1);
    }

    console.log('Generando atlas SDF (fallback Node.js puro, no MSDF real)...');
    console.log(`  Fuente:  ${FONT_PATH}`);
    console.log(`  Celda:   ${GLYPH_SIZE}x${GLYPH_SIZE} px, pxrange=${PXRANGE}`);

    const raw = fs.readFileSync(FONT_PATH);
    const arrayBuffer = raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength);
    const font = opentype.parse(arrayBuffer);
    const unitsPerEm = font.unitsPerEm;
    const emToRef = REFERENCE_EM_PX / unitsPerEm;

    const codepoints = [];
    for (let cp = CHARSET_START; cp <= CHARSET_END; cp++) codepoints.push(cp);

    const glyphInfos = codepoints.map(cp => {
        const glyph = font.charToGlyph(String.fromCodePoint(cp));
        const bbox = glyph.getBoundingBox();
        const hasOutline = glyph.path && glyph.path.commands && glyph.path.commands.length > 0;
        return { cp, glyph, bbox, hasOutline };
    });

    // Escala de rasterización uniforme: que quepan tanto la altura total
    // (ascender-descender) como el glifo más ancho dentro del área segura
    // de la celda (CELL - 2*PXRANGE), sin deformar el aspecto.
    const safeArea = GLYPH_SIZE - 2 * PXRANGE;
    const totalHeightUnits = font.ascender - font.descender;
    let maxWidthUnits = 1;
    for (const g of glyphInfos) {
        if (!g.hasOutline) continue;
        const w = g.bbox.x2 - g.bbox.x1;
        if (w > maxWidthUnits) maxWidthUnits = w;
    }
    const scaleV = safeArea / totalHeightUnits;
    const scaleH = safeArea / maxWidthUnits;
    const rasterScale = Math.min(scaleV, scaleH); // px del atlas por unidad de fuente

    const cellTopMargin = (GLYPH_SIZE - totalHeightUnits * rasterScale) / 2;
    const originY = cellTopMargin + font.ascender * rasterScale; // baseline, medido desde el tope de la celda

    // Grilla de empaquetado.
    const cols = Math.ceil(Math.sqrt(glyphInfos.length));
    const rows = Math.ceil(glyphInfos.length / cols);
    const atlasW = cols * GLYPH_SIZE;
    const atlasH = rows * GLYPH_SIZE;

    const png = new PNG({ width: atlasW, height: atlasH });
    png.data.fill(0);

    const glyphsJson = [];

    glyphInfos.forEach((info, idx) => {
        const col = idx % cols;
        const row = Math.floor(idx / cols);
        const cellLeftPx = col * GLYPH_SIZE;
        const cellTopPx = row * GLYPH_SIZE; // top-down, fila 0 = arriba de la imagen

        let edges = [];
        if (info.hasOutline) {
            const glyphWidthPx = (info.bbox.x2 - info.bbox.x1) * rasterScale;
            const leftMargin = (GLYPH_SIZE - glyphWidthPx) / 2;
            // originX/originYLocal son relativos a la celda (0..GLYPH_SIZE), NO al
            // atlas completo: rasterizeGlyphSDF muestrea en coordenadas locales de
            // celda, así que los edges deben vivir en ese mismo espacio local.
            const originXLocal = leftMargin - info.bbox.x1 * rasterScale;
            const originYLocal = originY; // ya es relativo al tope de la celda

            // glyph.path está en unidades de fuente crudas (sin escalar); lo aplanamos
            // ahí mismo y recién después transformamos cada punto a espacio de píxel
            // local de la celda (fx,fy en unidades de fuente, y-arriba -> px,py en
            // píxeles locales de la celda, y-abajo).
            const contours = flattenPathToContours(info.glyph.path, 10);
            for (const c of contours) {
                for (const p of c) {
                    const fx = p.x, fy = p.y;
                    p.x = originXLocal + fx * rasterScale;
                    p.y = originYLocal - fy * rasterScale;
                }
            }
            edges = contoursToEdges(contours);
        }

        const sdf = rasterizeGlyphSDF(edges, GLYPH_SIZE, PXRANGE);

        for (let y = 0; y < GLYPH_SIZE; y++) {
            for (let x = 0; x < GLYPH_SIZE; x++) {
                const signedDist = sdf[y * GLYPH_SIZE + x];
                const value = Math.max(0, Math.min(1, 0.5 + signedDist / PXRANGE));
                const byte = Math.round(value * 255);
                const px = cellLeftPx + x;
                const py = cellTopPx + y;
                const off = (atlasW * py + px) << 2;
                png.data[off] = byte;
                png.data[off + 1] = byte;
                png.data[off + 2] = byte;
                png.data[off + 3] = 255;
            }
        }

        const bottomOriginBottom = atlasH - (cellTopPx + GLYPH_SIZE);
        const bottomOriginTop = atlasH - cellTopPx;

        const advance = info.glyph.advanceWidth * emToRef;
        let planeBounds = { left: 0, bottom: 0, right: 0, top: 0 };
        if (info.hasOutline) {
            planeBounds = {
                left: info.bbox.x1 * emToRef,
                bottom: info.bbox.y1 * emToRef,
                right: info.bbox.x2 * emToRef,
                top: info.bbox.y2 * emToRef,
            };
        }

        glyphsJson.push({
            unicode: info.cp,
            advance,
            atlasBounds: {
                left: cellLeftPx,
                right: cellLeftPx + GLYPH_SIZE,
                bottom: bottomOriginBottom,
                top: bottomOriginTop,
            },
            planeBounds,
        });
    });

    fs.mkdirSync(OUTPUT_DIR, { recursive: true });
    const pngBuffer = PNG.sync.write(png);
    fs.writeFileSync(ATLAS_PNG, pngBuffer);

    const metaJson = {
        atlas: {
            type: 'sdf-single-channel-fallback', // honesto: no es 'msdf' real
            distanceRange: PXRANGE,
            size: GLYPH_SIZE,
            width: atlasW,
            height: atlasH,
        },
        glyphs: glyphsJson,
        note: 'Generado por tools/generate_msdf_atlas.js: SDF de un solo canal ' +
            'empaquetado en R=G=B (no MSDF multi-canal real). Ver header del script.',
    };
    fs.writeFileSync(ATLAS_JSON, JSON.stringify(metaJson, null, 2));

    // Round-trip: releer el PNG para confirmar que es válido y las dimensiones cierran.
    const verifyBuf = fs.readFileSync(ATLAS_PNG);
    const decoded = PNG.sync.read(verifyBuf);
    if (decoded.width !== atlasW || decoded.height !== atlasH) {
        console.error(`Round-trip de PNG falló: esperado ${atlasW}x${atlasH}, leído ${decoded.width}x${decoded.height}`);
        process.exit(1);
    }

    console.log('Atlas generado:');
    console.log(`  ${ATLAS_PNG} (${atlasW}x${atlasH}px, ${(pngBuffer.length / 1024).toFixed(1)} KB, round-trip OK)`);
    console.log(`  ${ATLAS_JSON} (${glyphsJson.length} glifos)`);
}

main();
