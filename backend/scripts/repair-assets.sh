#!/usr/bin/env bash
# One-shot fix for output extracted before the orientation and layout fixes:
#   1. `endfield repair` flips existing PNGs in place (local, no download).
#   2. `endfield update` re-extracts every bundle chunk into the container layout,
#      keeping the now-correct PNGs and decoding only missing ones, exports meshes as
#      .glb, extracts the BLC directories (tables, json, lua, audio, video), and removes
#      the legacy flat gamedata and textures/_ambiguous once everything succeeded.
# Both steps are resumable: rerun after an interruption.
#
# Usage: scripts/repair-assets.sh [--all] [extra `endfield update` args]
#   By default the engine level data (irradiance volumes, terrain, world streaming,
#   ~7 GB of runtime binaries) is skipped; --all includes it.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

# IV, Terrain, Streaming, DynamicStreaming.
LEVEL_DATA=A63D7E6A,F84BF5E6,C3442D43,23D53F5D
skip=(--skip-dir "$LEVEL_DATA")
need_gb=25
if [ "${1:-}" = "--all" ]; then
  skip=()
  need_gb=32
  shift
fi

# New gamedata is written before the legacy copy is deleted, so expect a transient peak.
mkdir -p ./output
free_gb=$(( $(df -Pk ./output | awk 'NR == 2 { print $4 }') / 1024 / 1024 ))
if (( free_gb < need_gb )); then
  echo "Only ${free_gb} GB free; this run may need about ${need_gb} GB."
  if [ ! -t 0 ]; then
    exit 1
  fi
  read -r -p "Continue anyway? [y/N] " reply
  [[ "$reply" =~ ^[Yy]$ ]] || exit 1
fi

cargo build --release --bin endfield -q
echo "== Flipping existing textures =="
./target/release/endfield repair

echo "== Re-extracting and downloading missing assets =="
exec scripts/auto-update.sh ${skip[@]+"${skip[@]}"} "$@"
