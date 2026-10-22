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
# Where this run's lines start in the log.
LOG_START=$(wc -c < "$LOG")

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

# The outcome of the last model build: `<ok|failed> <failures in a row> <epoch> <stamp>`,
# where the stamp is the update state it was built for.
MODELS_STATE=./data/models-state
# The lock `endfield update` and `endfield publish` take (an flock on this file).
STATE_LOCK="$STATE.lock"
# The lock of a model build where `flock` is missing (macOS).
MODELS_LOCK=./data/models.lock
# The exit code of a model build that found the lock taken.
MODELS_BUSY=75

# Whether a run that exited with $1 calls for the rigged models: it succeeded, did not
# step aside for another update, FLUORITE_MODELS is on, and the models were not built
# for the current state yet. Failed builds of one state back off: after the n-th in a
# row the next try waits 2^(n-1) hours, at most a day.
models_wanted() {
  local status failures at built current wait
  [ "$1" = 0 ] || return 1
  case "${FLUORITE_MODELS:-}" in
    1|true|yes|on) ;;
    *) return 1 ;;
  esac
  if tail -c +"$(( LOG_START + 1 ))" "$LOG" | grep -q '^another update is running'; then
    return 1
  fi
  # Without a state no update has completed: there is nothing to build the models from.
  if [ ! -f "$STATE" ]; then
    echo "models: skipped, no update state yet" >> "$LOG"
    return 1
  fi
  current=$(stamp "$STATE")
  read -r status failures at built 2>/dev/null < "$MODELS_STATE" || true
  [ "${built:-}" = "$current" ] || return 0
  [ "${status:-}" = failed ] || return 1
  wait=$(( 3600 * (1 << ((failures > 6 ? 6 : failures) - 1)) ))
  wait=$(( wait > 86400 ? 86400 : wait ))
  if (( $(date +%s) - at < wait )); then
    echo "models: skipped, $failures failed build(s) of this state; next try $(( (at + wait - $(date +%s)) / 60 )) min from now" >> "$LOG"
    return 1
  fi
}

# The rigged models are built one group at a time, and each group is published before
# the next one is exported: with S3_KEEP_LOCAL=0 the publish removes the local copies,
# so only one group is on the disk at once. The weapons go with the operators, whose
# character-screen data reads the weapon models from the disk.
MODEL_GROUPS=(operators npcs enemies)
MODELS_BIN=./target/release/endfield-models
MODELS_DIR=./output/models
MODELS_STORE=./output/models/.store
# The indexes each export or `clips` merges into. A publish removes them with the rest
# of the group, so a copy waits here (a dot directory is never published) for the next.
MODELS_KEEP=./output/models/.keep
MODELS_INDEXES=(index.json clips/summary.json)

# Build endfield-models and scan the chunks into the store (see `endfield-models` in the
# README). Everything goes to the log.
prepare_models() {
  echo "models: building endfield-models" >> "$LOG"
  if ! cargo build --release --features acl --bin endfield-models -q >> "$LOG" 2>&1; then
    echo "models: build failed" >> "$LOG"
    return 1
  fi
  if ! "$MODELS_BIN" scan --store "$MODELS_STORE" --jobs 2 >> "$LOG" 2>&1; then
    echo "models: scan failed" >> "$LOG"
    return 1
  fi
}

# Export one group of $MODEL_GROUPS with its clips.
export_models() {
  local exporter=("$MODELS_BIN" export --output "$MODELS_DIR" --store "$MODELS_STORE"
    --data-root ./output --jobs 2)
  local clips=("$MODELS_BIN" clips --store "$MODELS_STORE" --models "$MODELS_DIR" --jobs 2)
  local people=uiteam_idle,dialog_single_idle,idle
  echo "models: exporting $1" >> "$LOG"
  if ! case "$1" in
    operators)
      "${exporter[@]}" --filter prefabs/weapons &&
        "${exporter[@]}" --filter postmodels/characters --anims "$people" &&
        "${clips[@]}" --kind operator &&
        "$MODELS_BIN" charinfo --store "$MODELS_STORE" --models "$MODELS_DIR" --data-root ./output
      ;;
    npcs)
      "${exporter[@]}" --filter postmodels/npc --anims "$people" &&
        "${exporter[@]}" --filter /npc/major/ --anims "$people" &&
        "${clips[@]}" --kind npc
      ;;
    enemies)
      "${exporter[@]}" --filter postmodels/enemies --anims idle &&
        "${clips[@]}" --kind enemy
      ;;
  esac >> "$LOG" 2>&1; then
    echo "models: export of $1 failed" >> "$LOG"
    return 1
  fi
}

# Put back the indexes a publish removed, from the copy in $MODELS_KEEP or, before the
# first group, from the bucket: an export that starts from no index would publish one
# that lists only its own group.
restore_model_indexes() {
  local name base url prefix
  base="${S3_PUBLIC_BASE_URL:-}"
  prefix="${S3_PREFIX:-fluorite}"
  prefix="${prefix#/}"
  prefix="${prefix%/}"
  for name in "${MODELS_INDEXES[@]}"; do
    [ ! -f "$MODELS_DIR/$name" ] || continue
    mkdir -p "$(dirname "$MODELS_DIR/$name")"
    if [ -f "$MODELS_KEEP/$name" ]; then
      cp "$MODELS_KEEP/$name" "$MODELS_DIR/$name"
    elif [ -n "$base" ]; then
      url="${base%/}/${prefix:+$prefix/}models/$name"
      if curl -fsS --max-time 120 -o "$MODELS_DIR/$name.tmp" "$url" 2>> "$LOG"; then
        mv "$MODELS_DIR/$name.tmp" "$MODELS_DIR/$name"
      else
        rm -f "$MODELS_DIR/$name.tmp"
        echo "models: $url could not be read; $name starts empty" >> "$LOG"
      fi
    fi
  done
}

# Keep a copy of the indexes before a publish removes them.
keep_model_indexes() {
  local name
  for name in "${MODELS_INDEXES[@]}"; do
    [ -f "$MODELS_DIR/$name" ] || continue
    mkdir -p "$(dirname "$MODELS_KEEP/$name")"
    cp "$MODELS_DIR/$name" "$MODELS_KEEP/$name.tmp"
    mv "$MODELS_KEEP/$name.tmp" "$MODELS_KEEP/$name"
  done
}

# Run "$@" under the lock of the update state, so no update or publish removes its
# inputs while it reads them. With `flock` (Linux) it is the same flock the binary
# takes; without it (macOS) a directory only keeps two builds apart.
with_models_lock() {
  local rc=0
  if command -v flock > /dev/null; then
    exec 9>> "$STATE_LOCK"
    if ! flock -n 9; then
      exec 9>&-
      echo "models: skipped, an update or publish holds $STATE_LOCK" >> "$LOG"
      return "$MODELS_BUSY"
    fi
    # The children do not get fd 9: one that outlives the build would keep the lock.
    "$@" 9>&- || rc=$?
    exec 9>&-
    return "$rc"
  fi
  if ! mkdir "$MODELS_LOCK" 2>/dev/null; then
    if kill -0 "$(cat "$MODELS_LOCK/pid" 2>/dev/null)" 2>/dev/null; then
      echo "models: skipped, another run is building them" >> "$LOG"
      return "$MODELS_BUSY"
    fi
    rm -f "$MODELS_LOCK/pid"
    rmdir "$MODELS_LOCK" 2>/dev/null || true
    mkdir "$MODELS_LOCK" 2>/dev/null || return "$MODELS_BUSY"
  fi
  echo "$$" > "$MODELS_LOCK/pid"
  "$@" || rc=$?
  rm -f "$MODELS_LOCK/pid"
  rmdir "$MODELS_LOCK" 2>/dev/null || true
  return "$rc"
}

# The first group also builds the binary and fills the store.
build_model_group() {
  if [ "$1" = "${MODEL_GROUPS[0]}" ]; then
    prepare_models || return 1
  fi
  restore_model_indexes
  export_models "$1"
}

# Build and publish every group, then record the outcome. A failed group stops the run
# and counts as a failed build, so the backoff of `models_wanted` retries it; the exit
# code of the update is left alone.
update_models() {
  local rc=0 group current status failures at built
  current=$(stamp "$STATE")
  for group in "${MODEL_GROUPS[@]}"; do
    with_models_lock build_model_group "$group" || rc=$?
    [ "$rc" = 0 ] || break
    # The publish takes the lock itself, so it runs once the build has let go of it.
    keep_model_indexes
    if ! ./target/release/endfield publish --output ./output --state "$STATE" >> "$LOG" 2>&1; then
      echo "models: publish of $group failed; it is retried with the next build" >> "$LOG"
      rc=1
      break
    fi
    echo "models: $group published" >> "$LOG"
  done
  if [ "$rc" = "$MODELS_BUSY" ]; then
    return "$rc"
  fi
  if [ "$rc" != 0 ]; then
    read -r status failures at built 2>/dev/null < "$MODELS_STATE" || true
    if [ "${status:-}" != failed ] || [ "${built:-}" != "$current" ]; then
      failures=0
    fi
    echo "failed $(( failures + 1 )) $(date +%s) $current" > "$MODELS_STATE"
    return "$rc"
  fi
  restore_model_indexes
  echo "ok 0 $(date +%s) $current" > "$MODELS_STATE"
  echo "models: done" >> "$LOG"
}

if [ ! -t 1 ]; then
  before=$(stamp "$STATE")
  rc=0
  {
    cargo build --release --bin endfield -q || exit 1
    ./target/release/endfield update "$@"
  } >> "$LOG" 2>&1 || rc=$?
  if models_wanted "$rc"; then
    update_models || true
  fi
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
if models_wanted "$status"; then
  echo "Models: exporting…"
  models_rc=0
  update_models || models_rc=$?
  case "$models_rc" in
    0) echo "Models: done" ;;
    "$MODELS_BUSY") echo "Models: skipped, the lock is held, see $LOG" ;;
    *) echo "Models: failed, see $LOG" ;;
  esac
fi
if sync_wanted "$status" "$before"; then
  sync_bucket
fi
exit "$status"
