#!/usr/bin/env bash
# Runs auto-update.sh (with these arguments) forever, sleeping between runs; the pm2
# stand-in for a timer. A failed run is logged and the loop carries on.
#
#   scripts/update-loop.sh --no-voice --skip-dir Video
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/env.sh"
load_env "$HERE/../.env"
INTERVAL="${FLUORITE_UPDATE_INTERVAL:-900}"

# Low CPU and I/O priority, like the old systemd unit (Nice=10, idle I/O).
low=(nice -n 10)
command -v ionice >/dev/null && low+=(ionice -c 3)

while true; do
  echo "$(date '+%Y-%m-%dT%H:%M:%S%z') update starting"
  "${low[@]}" "$HERE/auto-update.sh" "$@"
  rc=$?
  echo "$(date '+%Y-%m-%dT%H:%M:%S%z') update exited with $rc; next in ${INTERVAL}s"
  sleep "$INTERVAL"
done
