#!/usr/bin/env bash
# Post-update smoke test for a running stack (API + web). Does not start or stop anything.
#
#   scripts/smoke.sh [--api-only | --web-only]
#
# Env: API (default http://127.0.0.1:8787), WEB (default http://localhost:3000).
# Exit status: 0 when every check passes, 1 otherwise.
set -euo pipefail

API="${API:-http://127.0.0.1:8787}"
WEB="${WEB:-http://localhost:3000}"
API="${API%/}"
WEB="${WEB%/}"
RUN_API=1
RUN_WEB=1

case "${1:-}" in
  "") ;;
  --api-only) RUN_WEB=0 ;;
  --web-only) RUN_API=0 ;;
  -h | --help)
    sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "usage: $0 [--api-only | --web-only]" >&2
    exit 2
    ;;
esac

TMP="$(mktemp -d "${TMPDIR:-/tmp}/fluorite-smoke.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
BODY="$TMP/body"
HDRS="$TMP/headers"
CODE=""
ERR=""
PASS=0
FAILED=0

now_ms() {
  perl -MTime::HiRes=time -e 'printf "%d", time * 1000' 2>/dev/null || echo "$(date +%s)000"
}

# fetch URL [max-seconds] [curl args...]: body -> $BODY, status -> $CODE.
fetch() {
  local url="$1" max="${2:-15}"
  shift 2 || shift $#
  CODE="$(curl -sS --max-time "$max" -D "$HDRS" -o "$BODY" -w '%{http_code}' "$@" "$url" 2>"$TMP/curl.err")" || {
    ERR="curl failed for $url: $(tr -d '\n' <"$TMP/curl.err")"
    CODE=000
    return 1
  }
}

# expect_status WANT...: succeeds when $CODE is one of the arguments.
expect_status() {
  local want
  for want in "$@"; do [ "$CODE" = "$want" ] && return 0; done
  ERR="status $CODE (wanted $*)"
  return 1
}

# jget EXPR: evaluate a Python expression against the parsed body as `d`; print the result.
jget() {
  python3 -c 'import json,sys
d = json.load(open(sys.argv[1]))
print(eval(sys.argv[2]))' "$BODY" "$1"
}

# jcheck EXPR MSG: body JSON `d` must satisfy EXPR.
jcheck() {
  if [ "$(jget "$1" 2>/dev/null || echo False)" != True ]; then
    ERR="$2"
    return 1
  fi
}

# api_json PATH EXPR MSG: GET, expect 200 and a truthy JSON expression.
api_json() {
  fetch "$API$1" 15 && expect_status 200 && jcheck "$2" "$3"
}

# head_bytes N: first N bytes of $BODY, non-printables as dots.
head_bytes() { LC_ALL=C head -c "$1" "$BODY" | LC_ALL=C tr -c '[:print:]' '.'; }

run() {
  local name="$1" fn="$2" t0 t1
  ERR=""
  t0="$(now_ms)"
  if "$fn"; then
    t1="$(now_ms)"
    printf 'OK    %-34s %6d ms\n' "$name" "$((t1 - t0))"
    PASS=$((PASS + 1))
  else
    t1="$(now_ms)"
    printf 'FAIL  %-34s %6d ms  %s\n' "$name" "$((t1 - t0))" "${ERR:-failed}"
    FAILED=$((FAILED + 1))
  fi
}

# ---- API checks -------------------------------------------------------------

OP_ID=""
OP_ICON=""

c_status() { api_json /api/status "bool(d.get('version'))" "no version"; }
c_catalog() { api_json /api/catalog "any(k['kind']=='operators' for k in d['kinds'])" "operators missing"; }

c_operator() {
  fetch "$API/api/catalog/operators" 20 && expect_status 200 || return 1
  OP_ID="$(jget "next((i['id'] for i in d['items'] if i.get('hasModel')), '')")"
  [ -n "$OP_ID" ] || { ERR="no operator with hasModel"; return 1; }
  fetch "$API/api/catalog/operators/$OP_ID" 15 && expect_status 200 || return 1
  jcheck "bool(d['model'].get('rigged'))" "$OP_ID: model.rigged not set" || return 1
  OP_ICON="$(jget "d.get('art',{}).get('icon','')")"
}

c_search() { api_json "/api/search?q=chen" "d['total']>0 and len(d['items'])>0" "no results"; }

c_tables() {
  local name
  api_json /api/tables "len(d['tables'])>0" "no tables" || return 1
  name="$(jget "d['tables'][0]['name']")"
  fetch "$API/api/tables/$name" 15 && expect_status 200 || return 1
  [ -s "$BODY" ] || { ERR="empty table $name"; return 1; }
}

c_story() {
  local id
  api_json /api/story "d['total']>0 and len(d['categories'])>0" "no stories" || return 1
  id="$(jget "d['categories'][0]['groups'][0]['dialogs'][0]")"
  api_json "/api/story/$id" "len(d['nodes'])>0" "dialog $id has no nodes"
}

c_audio() {
  api_json /api/audio "d['kinds']['external']>0 and 'ogg' in d['formats']" "summary looks empty"
}

AUDIO_URL=""
c_audio_entry() {
  # The first entry overall is a soundbank ref with no media; per the API, playable=true drops those.
  api_json "/api/audio/entries?limit=1&playable=true" "len(d['entries'])==1 and bool(d['entries'][0].get('audio'))" "first playable entry has no clip URL" || return 1
  AUDIO_URL="$(jget "d['entries'][0]['audio']['ogg']")"
}

c_audio_clip() {
  [ -n "$AUDIO_URL" ] || { ERR="no clip URL (entry check failed)"; return 1; }
  fetch "$API$AUDIO_URL" 30 && expect_status 200 || return 1
  case "$(head_bytes 4)" in
    OggS | RIFF) ;;
    *) ERR="clip starts with '$(head_bytes 4)', not OggS/RIFF"; return 1 ;;
  esac
}

c_audio_m4a() {
  [ -n "$AUDIO_URL" ] || { ERR="no clip URL (entry check failed)"; return 1; }
  fetch "$API${AUDIO_URL%.ogg}.m4a" 30 && expect_status 200 || return 1
  case "$(LC_ALL=C head -c 12 "$BODY" | LC_ALL=C tail -c 8 | LC_ALL=C tr -c '[:print:]' '.')" in
    ftyp*) ;;
    *) ERR="m4a has no ftyp box"; return 1 ;;
  esac
}

c_texture_range() {
  [ -n "$OP_ICON" ] || { ERR="no icon (operator check failed)"; return 1; }
  fetch "$API/files/textures/$OP_ICON" 15 -H 'Range: bytes=0-7' && expect_status 206 || return 1
  [ "$(LC_ALL=C wc -c <"$BODY" | tr -d ' ')" = 8 ] || { ERR="206 body is not 8 bytes"; return 1; }
  grep -qi '^content-range: bytes 0-7/' "$HDRS" || { ERR="no Content-Range header"; return 1; }
}

c_glb() {
  [ -n "$OP_ID" ] || { ERR="no operator (operator check failed)"; return 1; }
  fetch "$API/api/catalog/operators/$OP_ID" 15 && expect_status 200 || return 1
  local rigged
  rigged="$(jget "d['model']['rigged']")"
  fetch "$API/files/models/$rigged" 30 -H 'Range: bytes=0-3' && expect_status 200 206 || return 1
  [ "$(head_bytes 4)" = glTF ] || { ERR="rigged GLB starts with '$(head_bytes 4)'"; return 1; }
}

c_traversal() {
  fetch "$API/files/gamedata/%2e%2e/%2e%2e/etc/passwd" 10 --path-as-is || return 1
  expect_status 400 404 || return 1
  ! grep -q '^root:' "$BODY" || { ERR="leaked /etc/passwd"; return 1; }
}

# ---- Web checks -------------------------------------------------------------

# These two routes 307 to their canonical search-param URL, so ask for it directly.
web_page() {
  fetch "$WEB$1" 20 && expect_status 200 || return 1
  grep -qi '<title' "$BODY" || { ERR="no <title in response"; return 1; }
}

w_home() { web_page /; }
w_studio() { web_page /studio; }
w_catalog() { web_page /catalog/operators; }
w_operator() {
  [ -n "$OP_ID" ] || OP_ID="$(fetch "$API/api/catalog/operators" 20 && jget "d['items'][0]['id']")"
  web_page "/catalog/operators/$OP_ID"
}
w_story() { web_page /story; }
w_audio() { web_page /audio; }
w_textures() { web_page '/textures?dir='; }
w_tables() { web_page /tables; }
w_browse() { web_page '/browse/gamedata?path='; }

# ---- Run --------------------------------------------------------------------

if [ "$RUN_API" = 1 ]; then
  echo "API $API"
  run "api /api/status" c_status
  run "api /api/catalog" c_catalog
  run "api operator detail (rigged)" c_operator
  run "api /api/search" c_search
  run "api tables list + table" c_tables
  run "api story list + dialog" c_story
  run "api /api/audio" c_audio
  run "api audio entry" c_audio_entry
  run "api audio clip ogg (30s)" c_audio_clip
  run "api audio clip m4a (30s)" c_audio_m4a
  run "api texture Range 206" c_texture_range
  run "api rigged GLB magic" c_glb
  run "api path traversal" c_traversal
fi

if [ "$RUN_WEB" = 1 ]; then
  echo "WEB $WEB"
  run "web /" w_home
  run "web /studio" w_studio
  run "web /catalog/operators" w_catalog
  run "web operator detail" w_operator
  run "web /story" w_story
  run "web /audio" w_audio
  run "web /textures" w_textures
  run "web /tables" w_tables
  run "web /browse/gamedata" w_browse
fi

echo
echo "$PASS passed, $FAILED failed"
[ "$FAILED" -eq 0 ]
