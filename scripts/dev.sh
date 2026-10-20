#!/usr/bin/env bash
# Development: fluorite-api (release build) + the Vite dev server with HMR.
# Reuses an API that is already answering on FLUORITE_API_URL. Ctrl-C stops both.
#
#   scripts/dev.sh                 # API on :8787, frontend on :3000
#   PORT=3001 scripts/dev.sh       # another frontend port
set -euo pipefail
source "$(dirname "$0")/lib.sh"
trap cleanup INT TERM EXIT

PORT="${PORT:-3000}"
command -v bun >/dev/null || die "bun is not installed (https://bun.sh)"
[ -d "$FRONTEND/node_modules" ] || (cd "$FRONTEND" && bun install)

start_api

log "starting frontend dev server on http://localhost:$PORT"
(cd "$FRONTEND" && exec bunx vite dev --port "$PORT" --strictPort) &
PIDS+=($!)

wait_any
