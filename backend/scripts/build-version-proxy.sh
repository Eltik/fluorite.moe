#!/usr/bin/env bash
# Builds the qavif.dll Qt imageformat plugin loader.
#
# Minimal build: no CRT, no TLS, no debug sections.
# Phase 1: IL2CPP Survey — resolves IL2CPP API from GameAssembly.dll,
# enumerates classes, dumps metadata for target classes to JSON.
# Phase 2: Stat Capture — inline hooks on known RVAs for character stats.
set -euo pipefail

cd "$(dirname "$0")/../version-dll-proxy"

x86_64-w64-mingw32-gcc -shared -nostartfiles -nodefaultlibs \
    -s -O2 -e DllEntry \
    -o qavif.dll proxy.c \
    -lkernel32

mkdir -p ../data/game-binaries
cp qavif.dll ../data/game-binaries/qavif.dll
ls -lh qavif.dll
echo "OK: data/game-binaries/qavif.dll"
