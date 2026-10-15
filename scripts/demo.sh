#!/bin/sh
# Records the screenshots and demo video in docs/media from sample data.
# The app runs with a throw-away home folder and settings, shows only the startup
# volume, and captures only its own windows, so nothing personal ends up in the media.
#
# Usage: scripts/demo.sh            (needs ffmpeg; keep the display awake while it runs)
set -eu
cd "$(dirname "$0")/.."

DEMO="/Users/Shared/FerrousDemo"
OUT="$PWD/docs/media"
FFMPEG=$(command -v ffmpeg) || { echo "ffmpeg is required (brew install ffmpeg)"; exit 1; }

scripts/bundle.sh >/dev/null
rm -rf "$DEMO"
python3 -I scripts/make_demo_data.py "$DEMO"
mkdir -p "$OUT"
rm -f "$OUT"/*.png "$OUT"/*.mp4 "$OUT"/*.gif

open -n -W \
  --env HOME="$DEMO/home" \
  --env FC_CONFIG_DIR="$DEMO/config" \
  --env FC_DEMO="$OUT" \
  --env FC_FFMPEG="$FFMPEG" \
  --stderr "$DEMO/demo.log" --stdout "$DEMO/demo.log" \
  "dist/Ferrous Commander.app"

cat "$DEMO/demo.log"
grep -q "DEMO OK" "$DEMO/demo.log" || { echo "demo failed"; exit 1; }

# Animated preview for the README (first 24 s, 960 px wide).
"$FFMPEG" -y -loglevel error -t 24 -i "$OUT/ferrous-commander-demo.mp4" \
  -vf "fps=15,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=4" \
  "$OUT/ferrous-commander-demo.gif"

rm -rf "$DEMO"
ls -lh "$OUT"
