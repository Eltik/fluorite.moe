#!/usr/bin/env bash
# Production: fluorite-api + the built frontend (`.output/`, Bun) on one public origin.
# Builds whatever is missing or out of date first. Ctrl-C / SIGTERM stops both.
#
#   scripts/serve.sh                      # web on :3000, API on 127.0.0.1:8787
#   PORT=8080 HOST=127.0.0.1 scripts/serve.sh
#   scripts/serve.sh --rebuild            # force both builds
#
# The browser talks to the API through the web server's same-origin proxy unless
# FLUORITE_PUBLIC_API_URL points at a separately published API.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
trap cleanup INT TERM EXIT

REBUILD=
[ "${1:-}" = "--rebuild" ] && REBUILD=force
export PORT="${PORT:-3000}"
export NODE_ENV=production

command -v bun >/dev/null || die "bun is not installed (https://bun.sh)"
# Bun < 1.3.14 cannot parse React's server renderer once bundled (labelled-statement bug).
bun_ok=$(bun -e 'const [a,b,c]=Bun.version.split(".").map(Number);console.log(a>1||(a===1&&(b>3||(b===3&&c>=14)))?1:0)')
[ "$bun_ok" = 1 ] || die "bun $(bun --version) is too old; the SSR server needs >= 1.3.14 (run: bun upgrade)"

[ -n "$REBUILD" ] && build_api force
start_api

out="$FRONTEND/.output/nitro.json"
if [ -n "$REBUILD" ] || [ ! -f "$out" ] ||
  [ -n "$(find "$FRONTEND/src" "$FRONTEND/public" "$FRONTEND/package.json" "$FRONTEND/vite.config.ts" "$FRONTEND/bun.lock" -newer "$out" -print -quit)" ]; then
  log "building frontend"
  (cd "$FRONTEND" && bun install --frozen-lockfile && bun run build)
fi

log "serving http://${HOST:-0.0.0.0}:$PORT (API upstream: $FLUORITE_API_URL)"
(cd "$FRONTEND" && exec bun .output/server/index.mjs) &
PIDS+=($!)

wait_any
