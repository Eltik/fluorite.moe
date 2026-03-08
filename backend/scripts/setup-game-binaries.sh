#!/usr/bin/env bash
# Downloads game DLLs from the launcher API into data/game-binaries/
set -euo pipefail

cd "$(dirname "$0")/.."
cargo run --bin gameassembly-extract --release -- GameAssembly.dll HGP.dll UnityPlayer.dll

# Verify
for f in data/game-binaries/GameAssembly.dll data/game-binaries/HGP.dll data/game-binaries/UnityPlayer.dll; do
  [ -f "$f" ] && echo "OK: $f" || echo "MISSING: $f"
done
