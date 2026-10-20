#!/usr/bin/env bash
# Wrapper for `endfield health`. Prints the checks and, when any is not OK, raises a macOS
# notification and (with FLUORITE_ALERT_WEBHOOK set) posts to a Discord or Slack webhook.
# Exits with the health exit code: 0 OK, 1 WARN, 2 FAIL.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

json_escape() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  s=${s//$'\r'/}
  s=${s//$'\t'/\\t}
  printf '%s' "$s"
}

alert() {
  local summary=$1
  if command -v osascript >/dev/null 2>&1; then
    local escaped=${summary//\\/\\\\}
    escaped=${escaped//\"/\\\"}
    osascript -e "display notification \"$escaped\" with title \"fluorite.moe health\"" \
      >/dev/null 2>&1 || true
  fi
  if [ -n "${FLUORITE_ALERT_WEBHOOK:-}" ]; then
    local body
    body=$(json_escape "$summary")
    # Discord reads `content`, Slack reads `text`; each ignores the other.
    curl -fsS -m 20 -H 'Content-Type: application/json' \
      -d "{\"content\": \"$body\", \"text\": \"$body\"}" \
      "$FLUORITE_ALERT_WEBHOOK" >/dev/null || echo "health-check: webhook post failed" >&2
  fi
}

if ! build_log=$(cargo build --release --bin endfield -q 2>&1); then
  printf '%s\n' "$build_log"
  alert "fluorite.moe health on $(hostname -s): FAIL: cargo build failed"
  exit 2
fi

set +e
report=$(./target/release/endfield health "$@" 2>&1)
rc=$?
set -e
printf '%s\n' "$report"

if [ "$rc" -ne 0 ]; then
  problems=$(printf '%s\n' "$report" | grep -v '^OK: ' || true)
  alert "fluorite.moe health on $(hostname -s):
${problems:-endfield health exited with $rc}"
fi
exit "$rc"
