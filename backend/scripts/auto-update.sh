#!/usr/bin/env bash
# Wrapper for `endfield update`. From a terminal it draws a live progress bar;
# under cron (no TTY) it only appends to the log.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE/.."
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

. "$HERE/env.sh"
load_env "$HERE/../.env"

LOG=./data/logs/update.log
mkdir -p ./data/logs
if [ -f "$LOG" ] && [ "$(wc -c < "$LOG")" -gt 10485760 ]; then
  mv "$LOG" "$LOG.1"
fi
printf '\n===== %s =====\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" >> "$LOG"

STATE=./data/update-state.json
stamp() {
  if [ -f "$1" ]; then
    cksum < "$1" | awk '{ print $1, $2 }'
  else
    echo "none"
  fi
}

sync_bucket() {
  [ -n "${S3_ENDPOINT:-${CLOUDFLARE_S3_API:-}}" ] || return 0
  [ -n "${S3_BULK_SYNC:-}" ] || return 0
  ./scripts/sync-s3.sh >> "$LOG" 2>&1 || echo "s3 sync failed, see $LOG" >> "$LOG"
}

# Whether a run that exited with $1 calls for a sync: it succeeded, and the state
# file changed from $2 (or S3_SYNC_ALWAYS is set).
sync_wanted() {
  [ "$1" = 0 ] || return 1
  [ "$2" != "$(stamp "$STATE")" ] || [ -n "${S3_SYNC_ALWAYS:-}" ]
}

if [ ! -t 1 ]; then
  before=$(stamp "$STATE")
  rc=0
  {
    cargo build --release --bin endfield -q || exit 1
    ./target/release/endfield update "$@"
  } >> "$LOG" 2>&1 || rc=$?
  if sync_wanted "$rc" "$before"; then
    sync_bucket
  fi
  exit "$rc"
fi

# ── Interactive: render @progress lines as a bar ─────────

BAR_WIDTH=30
SPIN='|/-\'
tick=0
phase="build" done_n=0 total=0 label="compiling"
phase_start=$SECONDS

fmt_secs() {
  printf '%d:%02d' $(( $1 / 60 )) $(( $1 % 60 ))
}

fmt_gb() {
  printf '%d.%d' $(( $1 / 1024 )) $(( $1 % 1024 * 10 / 1024 ))
}

phase_title() {
  case "$1" in
    build) echo "Building" ;;
    resolve) echo "Checking" ;;
    chk) echo "Assets  " ;;
    sort) echo "Sorting " ;;
    blc) echo "Tables  " ;;
    schema) echo "Schema  " ;;
    *) echo "$1" ;;
  esac
}

draw() {
  local cols line bar="" i filled pct eta="" elapsed
  cols=$(tput cols 2>/dev/null || echo 80)
  elapsed=$(( SECONDS - phase_start ))
  if (( total > 0 )); then
    filled=$(( done_n * BAR_WIDTH / total ))
    pct=$(( done_n * 100 / total ))
    for (( i = 0; i < BAR_WIDTH; i++ )); do
      if (( i < filled )); then bar+="█"; else bar+="░"; fi
    done
    if (( done_n > 0 && done_n < total )); then
      eta=" eta $(fmt_secs $(( elapsed * (total - done_n) / done_n )))"
    fi
    local counts="$done_n/$total"
    case "$phase" in
      chk|sort) counts="$(fmt_gb "$done_n")/$(fmt_gb "$total") GB" ;;
    esac
    line="$(phase_title "$phase") $bar $(printf '%3d' "$pct")% $counts $(fmt_secs "$elapsed")$eta"
  else
    line="$(phase_title "$phase") ${SPIN:tick % 4:1} $(fmt_secs "$elapsed")"
  fi
  # The label goes last and is clipped so the line never wraps (bar glyphs count as 1 column).
  local room=$(( cols - ${#line} - 3 ))
  if (( room > 4 )) && [ -n "$label" ]; then
    line+="  ${label:0:room}"
  fi
  printf '\r\033[K%s' "$line"
}

say() {
  printf '\r\033[K%s\n' "$1"
  draw
}

trap 'printf "\r\033[K"; tput cnorm 2>/dev/null || true' EXIT
tput civis 2>/dev/null || true

draw
if ! cargo build --release --bin endfield -q >> "$LOG" 2>&1; then
  say "build failed, see $LOG"
  exit 1
fi

before=$(stamp "$STATE")

# A ticker shares the pipe so the bar keeps moving while the binary is busy;
# the binary's exit status comes through as a final `@exit` line.
status=0
{
  (while sleep 1; do echo "@tick"; done) &
  ticker=$!
  set +e
  ./target/release/endfield update --progress "$@" 2>&1
  rc=$?
  set -e
  kill "$ticker" 2>/dev/null || true
  echo "@exit $rc"
} | {
  rc=1
  while IFS= read -r line; do
    case "$line" in
      "@tick")
        tick=$(( tick + 1 ))
        draw
        ;;
      "@exit "*)
        rc=${line#@exit }
        ;;
      "@progress "*)
        read -r _ p d t l <<< "$line"
        if [ "$p" != "$phase" ]; then phase_start=$SECONDS; fi
        phase=$p done_n=$d total=$t label=${l:-}
        draw
        ;;
      *)
        echo "$line" >> "$LOG"
        case "$line" in
          *error*|*failed*|*failures*|"up to date"*|"update available"*|"another update"*)
            say "$line" ;;
        esac
        ;;
    esac
  done
  exit "$rc"
} || status=$?

printf '\r\033[K'
if sync_wanted "$status" "$before"; then
  sync_bucket
fi
exit "$status"
