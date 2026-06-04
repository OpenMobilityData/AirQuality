#!/usr/bin/env bash
# Build and deploy AirQuality to the production VPS.
#
# Served at https://mtl-aq.org — hosted on the same server as bikestat.org
# under its own vhost. Override the destination with AIRQUALITY_REMOTE /
# AIRQUALITY_DEST.
#
# Pass --download to fetch the full RSQA archive (1986–2024) into data-src/
# first. The compact map/daily outputs are committed, but the large hourly
# per-year files are not (see .gitignore); regenerate them from the archive
# before deploying if you want the Hour interval available on the server.
set -euo pipefail

REMOTE="${AIRQUALITY_REMOTE:-rhoge@bikestat.org}"
DEST="${AIRQUALITY_DEST:-/var/www/mtl-aq/}"

cd "$(dirname "$0")/.."

# Make sure cargo / trunk are on PATH when invoked from a non-login shell.
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

if [ "${1:-}" = "--download" ]; then
    scripts/fetch-archive.sh
fi

# Regenerate the compact data files from data-src/ if the raw inputs are present
# (skipped on hosts that only have the committed static/data/ outputs).
if ls data-src/rsqa-multi-polluants*.csv >/dev/null 2>&1; then
    python3 scripts/preprocess.py
fi

trunk build --release

rsync -av --delete dist/ "${REMOTE}:${DEST}"
