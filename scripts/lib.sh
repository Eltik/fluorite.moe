# Shared helpers for dev.sh and serve.sh. Sourced, not executed.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKEND="$ROOT/backend"
FRONTEND="$ROOT/frontend"
export PATH="$PATH:$HOME/.cargo/bin:$HOME/.bun/bin:/opt/homebrew/bin:/usr/local/bin"

# API defaults (see backend/src/api/config.rs); anything already exported wins.
export FLUORITE_API_ADDR="${FLUORITE_API_ADDR:-127.0.0.1:8787}"
export FLUORITE_OUTPUT="${FLUORITE_OUTPUT:-$BACKEND/output}"
# Where the web server reaches the API.
export FLUORITE_API_URL="${FLUORITE_API_URL:-http://$FLUORITE_API_ADDR}"

PIDS=()

log() { printf '\033[2m[%s]\033[0m %s\n' "$(basename "$0" .sh)" "$*"; }
die() { log "error: $*" >&2; exit 1; }

api_up() { curl -fsS -o /dev/null --max-time 2 "$FLUORITE_API_URL/api/status"; }

# Stop every child we started (and their process groups' children, e.g. vite).
cleanup() {
  trap - INT TERM EXIT
  for pid in "${PIDS[@]:-}"; do
    [ -n "$pid" ] || continue
    pkill -TERM -P "$pid" 2>/dev/null || true
    kill -TERM "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}

# Block until any child exits (bash 3.2 on macOS has no `wait -n`), then return its status.
wait_any() {
  while :; do
    for pid in "${PIDS[@]}"; do
      if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null
        return $?
      fi
    done
    sleep 1
  done
}

# Build the API binary when it is missing or older than its sources.
build_api() {
  local bin="${CARGO_TARGET_DIR:-$BACKEND/target}/release/fluorite-api"
  if [ "${1:-}" = force ] || [ ! -x "$bin" ] ||
    [ -n "$(find "$BACKEND/src" "$BACKEND/Cargo.toml" "$BACKEND/Cargo.lock" -newer "$bin" -print -quit)" ]; then
    log "building fluorite-api (release)"
    (cd "$BACKEND" && cargo build --release --bin fluorite-api)
  fi
  API_BIN="$bin"
}

start_api() {
  if api_up; then
    log "API already answering at $FLUORITE_API_URL; not starting another"
    return
  fi
  [ -d "$FLUORITE_OUTPUT" ] || die "FLUORITE_OUTPUT=$FLUORITE_OUTPUT does not exist (run endfield update first)"
  build_api
  log "starting fluorite-api on $FLUORITE_API_ADDR (output: $FLUORITE_OUTPUT)"
  "$API_BIN" &
  PIDS+=($!)
  for _ in $(seq 1 60); do
    api_up && return
    kill -0 "${PIDS[${#PIDS[@]}-1]}" 2>/dev/null || die "fluorite-api exited during startup"
    sleep 0.5
  done
  log "warning: API not answering after 30s; continuing"
}
