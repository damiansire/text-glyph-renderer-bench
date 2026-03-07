#!/usr/bin/env bash
# generate_msdf_atlas.sh — Genera el atlas MSDF offline usando msdf-atlas-gen
#
# Prerequisitos:
#   brew install msdf-atlas-gen
#   (o compilar desde https://github.com/Chlumsky/msdf-atlas-gen)
#
# Uso:
#   cd poc-1d-webgpu-msdf
#   bash tools/generate_msdf_atlas.sh
#
# Salida:
#   assets/inter_msdf_atlas.png   — atlas de texturas MSDF (RGBA, 2048×2048)
#   assets/inter_msdf_atlas.json  — metadata: {glyphs:[{unicode, x, y, w, h, advance, ...}]}

set -euo pipefail

FONT_FILE="${1:-../shared/fonts/InterVariable.ttf}"
OUTPUT_DIR="assets"
ATLAS_PNG="$OUTPUT_DIR/inter_msdf_atlas.png"
ATLAS_JSON="$OUTPUT_DIR/inter_msdf_atlas.json"
ATLAS_SIZE=2048
GLYPH_SIZE=48   # pixels per glyph cell in the atlas (good for 8–72pt rendering)

mkdir -p "$OUTPUT_DIR"

echo "🔤 Generando atlas MSDF..."
echo "   Fuente:    $FONT_FILE"
echo "   Atlas:     ${ATLAS_SIZE}×${ATLAS_SIZE} px"
echo "   Glifo:     ${GLYPH_SIZE}×${GLYPH_SIZE} px"

msdf-atlas-gen \
  -font "$FONT_FILE" \
  -type msdf \
  -format png \
  -imageout "$ATLAS_PNG" \
  -json "$ATLAS_JSON" \
  -size "$GLYPH_SIZE" \
  -pxrange 4 \
  -dimensions "$ATLAS_SIZE" "$ATLAS_SIZE" \
  -charset "charset_latin_cjk_basic.txt" \
  -yorigin bottom

echo "✅ Atlas generado:"
echo "   $ATLAS_PNG"
echo "   $ATLAS_JSON"
echo ""
echo "ℹ️  Para CJK completo (20k glifos) aumentar GLYPH_SIZE a 24-32px y"
echo "   usar atlas de 4096×4096 (almacenado en Git LFS)."
