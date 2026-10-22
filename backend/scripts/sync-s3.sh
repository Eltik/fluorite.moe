#!/usr/bin/env bash
# Mirror the configured roots of the extraction output into an S3-compatible bucket.
# The bucket mirrors the local layout: the key of `textures/ui/logo.png` is
# `fluorite/textures/ui/logo.png` (below S3_PREFIX), so the sync is a plain
# directory mirror.
#
# The unpacker publishes the new files on its own (`endfield publish`); this is
# the bulk fill of a bucket that holds tens of thousands of files.
#
# Reads the same variables as the API, from `backend/.env` or the environment (the
# environment wins):
#   S3_ENDPOINT, S3_BUCKET, S3_ACCESS_KEY_ID, S3_SECRET_ACCESS_KEY   (required)
#   S3_TYPE, S3_REGION, S3_PREFIX, S3_ROOTS, S3_PATH_STYLE, S3_THUMBS,
#   S3_CACHE_CONTROL, FLUORITE_OUTPUT, FLUORITE_CACHE_DIR
# For Cloudflare R2 the endpoint and the credentials come from the dashboard names
# instead: CLOUDFLARE_S3_API, CLOUDFLARE_API_ACCESS_KEY, CLOUDFLARE_API_SECRET_KEY.
#
# Usage: scripts/sync-s3.sh [--dry-run] [--delete] [extra rclone flags]
#   --dry-run  report what would change, upload nothing
#   --delete   remove bucket objects that are gone locally (a full mirror)
#   By default the sync only adds and overwrites: an interrupted extraction can
#   never delete the good copy in the bucket.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE/.."
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

LOG=./data/logs/sync-s3.log
LOCK=./data/sync-s3.lock
mkdir -p ./data/logs
if [ -f "$LOG" ] && [ "$(wc -c < "$LOG")" -gt 10485760 ]; then
  mv "$LOG" "$LOG.1"
fi

DRY_RUN=0
DELETE=0
RCLONE_ARGS=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --delete) DELETE=1 ;;
    *) RCLONE_ARGS+=("$arg") ;;
  esac
done

say() {
  local line
  line="$(date '+%Y-%m-%dT%H:%M:%S%z') $*"
  printf '%s\n' "$line"
  printf '\n%s\n' "$line" >> "$LOG"
}

. "$HERE/env.sh"
load_env "$HERE/../.env"

ENDPOINT=${S3_ENDPOINT:-${CLOUDFLARE_S3_API:-}}
BUCKET=${S3_BUCKET:-}
ACCESS=${S3_ACCESS_KEY_ID:-${CLOUDFLARE_API_ACCESS_KEY:-}}
SECRET=${S3_SECRET_ACCESS_KEY:-${CLOUDFLARE_API_SECRET_KEY:-}}
PREFIX=${S3_PREFIX:-fluorite}
ROOTS=${S3_ROOTS:-textures,meshes,models}
PATH_STYLE=${S3_PATH_STYLE:-1}
THUMBS=${S3_THUMBS:-1}
CACHE_CONTROL=${S3_CACHE_CONTROL:-}
OUTPUT=${FLUORITE_OUTPUT:-./output}
CACHE_DIR=${FLUORITE_CACHE_DIR:-$OUTPUT/.cache}

missing=()
for pair in "S3_ENDPOINT:$ENDPOINT" "S3_BUCKET:$BUCKET" \
  "S3_ACCESS_KEY_ID:$ACCESS" "S3_SECRET_ACCESS_KEY:$SECRET"; do
  [ -n "${pair#*:}" ] || missing+=("${pair%%:*}")
done
if [ ${#missing[@]} -gt 0 ]; then
  say "not configured, nothing to sync (missing: ${missing[*]})"
  exit 0
fi
if ! command -v rclone >/dev/null 2>&1; then
  say "rclone is not installed: brew install rclone"
  exit 1
fi

rclone_provider() {
  case "$1" in
    cloudflare|r2) echo Cloudflare ;;
    b2|backblaze) echo Other ;;
    digitalocean|spaces) echo DigitalOcean ;;
    wasabi) echo Wasabi ;;
    aws|amazon) echo Amazon ;;
    seaweedfs|seaweed) echo SeaweedFS ;;
    minio) echo Minio ;;
    s3|other|generic) echo Other ;;
    *) echo "" ;;
  esac
}

guess_provider() {
  case "$1" in
    *r2.cloudflarestorage.com*) echo Cloudflare ;;
    *backblazeb2.com*) echo Other ;;
    *digitaloceanspaces.com*) echo DigitalOcean ;;
    *wasabisys.com*) echo Wasabi ;;
    *amazonaws.com*) echo Amazon ;;
    *seaweed*) echo SeaweedFS ;;
    *localhost*|*127.0.0.1*|*minio*) echo Minio ;;
    *) echo Other ;;
  esac
}

PROVIDER=$(rclone_provider "${S3_TYPE:-}")
[ -n "$PROVIDER" ] || PROVIDER=$(guess_provider "$ENDPOINT")

host_of() {
  echo "$1" | sed -e 's|^[a-zA-Z]*://||' -e 's|/.*$||' -e 's|:[0-9]*$||'
}

guess_region() {
  local host="$1"
  case "$host" in
    *r2.cloudflarestorage.com*) echo auto ;;
    *backblazeb2.com*) echo "$(echo "$host" | cut -d. -f2)" ;;
    *digitaloceanspaces.com*) echo "$(echo "$host" | cut -d. -f2)" ;;
    *) echo us-east-1 ;;
  esac
}

if [ -z "${S3_REGION:-}" ]; then
  REGION=$(guess_region "$(host_of "$ENDPOINT")")
else
  REGION=$S3_REGION
fi

PREFIX=${PREFIX#/}
PREFIX=${PREFIX%/}
[ -n "$PREFIX" ] && PREFIX="$PREFIX/"

root_dir() {
  case "$1" in
    textures) echo "textures" ;;
    meshes) echo "meshes" ;;
    models) echo "models" ;;
    gamedata) echo "gamedata" ;;
    tables) echo "decrypted/Table/TableCfg" ;;
    jsondata) echo "decrypted/JsonData/Json" ;;
    lua) echo "decrypted/Lua/LuaScripts" ;;
    audio) echo "decrypted/Audio" ;;
    *) return 1 ;;
  esac
}

if ! mkdir "$LOCK" 2>/dev/null; then
  stale=0
  if [ -d "$LOCK" ]; then
    mtime=$(stat -f %m "$LOCK" 2>/dev/null || stat -c %Y "$LOCK" 2>/dev/null || echo 0)
    age=$(( $(date +%s) - mtime ))
    [ "$age" -gt 21600 ] && stale=1
  fi
  if [ "$stale" = 1 ]; then
    say "removing the stale lock"
    rmdir "$LOCK" 2>/dev/null || true
    mkdir "$LOCK" 2>/dev/null || {
      say "another sync is running"
      exit 0
    }
  else
    say "another sync is running, skipping"
    exit 0
  fi
fi

CONFIG=$(mktemp "${TMPDIR:-/tmp}/fluorite-rclone.XXXXXX")
chmod 600 "$CONFIG"
trap 'rm -f "$CONFIG"; rmdir "$LOCK" 2>/dev/null || true' EXIT
cat > "$CONFIG" <<EOF
[fluorite]
type = s3
provider = $PROVIDER
access_key_id = $ACCESS
secret_access_key = $SECRET
region = $REGION
endpoint = $ENDPOINT
force_path_style = $PATH_STYLE
EOF

RCLONE=(rclone --config "$CONFIG")
if [ "$DRY_RUN" = 1 ]; then
  RCLONE+=(--dry-run)
fi

COMMON=(--fast-list --s3-no-check-bucket --transfers 16 --checkers 32
  --exclude ".*" --exclude "*.tmp" --exclude "*.mpk" --exclude "blc_metadata.txt"
  --stats 30s --stats-one-line --log-level INFO)

if [ -n "$CACHE_CONTROL" ]; then
  COMMON+=(--header-upload "Cache-Control: $CACHE_CONTROL")
fi

say "provider=$PROVIDER bucket=$BUCKET prefix=${PREFIX:-none} roots=$ROOTS delete=$DELETE"

if ! "${RCLONE[@]}" lsd "fluorite:${BUCKET}" >/dev/null 2>&1; then
  say "the bucket is not reachable, check the endpoint and the credentials"
  exit 1
fi

failed=0
for root in ${ROOTS//,/ }; do
  root=${root// /}
  [ -n "$root" ] || continue
  rel=$(root_dir "$root") || {
    say "unknown root '$root', skipping"
    continue
  }
  src="$OUTPUT/$rel"
  if [ ! -d "$src" ]; then
    say "$root: no local directory ($src), skipping"
    continue
  fi
  dst="fluorite:${BUCKET}/${PREFIX}${rel}"
  mode=copy
  [ "$DELETE" = 1 ] && mode=sync
  say "$root: $mode $src -> $dst"
  "${RCLONE[@]}" "$mode" "$src" "$dst" "${COMMON[@]}" \
    ${RCLONE_ARGS[@]+"${RCLONE_ARGS[@]}"} 2>&1 | tee -a "$LOG" || failed=1
done

if [ "$THUMBS" = 1 ] && [ -d "$CACHE_DIR/thumbs" ]; then
  mode=copy
  [ "$DELETE" = 1 ] && mode=sync
  say "thumbs: $mode $CACHE_DIR/thumbs -> fluorite:${BUCKET}/${PREFIX}thumbs"
  "${RCLONE[@]}" "$mode" "$CACHE_DIR/thumbs" "fluorite:${BUCKET}/${PREFIX}thumbs" \
    "${COMMON[@]}" ${RCLONE_ARGS[@]+"${RCLONE_ARGS[@]}"} 2>&1 | tee -a "$LOG" || failed=1
fi

if [ "$failed" != 0 ]; then
  say "the sync failed, see $LOG"
  exit 1
fi
say "done"
