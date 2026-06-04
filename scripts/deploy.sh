#!/usr/bin/env bash
# Build and deploy AirQuality to the production VPS.
#
# Served at https://mtl-aq.org — hosted on the same server as bikestat.org
# under its own vhost. Override the destination with AIRQUALITY_REMOTE /
# AIRQUALITY_DEST.
#
# Modes:
#   ./scripts/deploy.sh             Code-only: build + rsync the app, leaving the
#                                   data already on the server untouched. Fast —
#                                   does NOT re-run preprocess or re-send the large
#                                   data tiers (the hourly series alone is ~250 MB).
#   ./scripts/deploy.sh --data      Also regenerate static/data from data-src/ and
#                                   sync the data (content-compared, so unchanged
#                                   files aren't re-sent).
#   ./scripts/deploy.sh --download  Fetch/refresh the raw archive first, then --data.
set -euo pipefail

REMOTE="${AIRQUALITY_REMOTE:-rhoge@bikestat.org}"
DEST="${AIRQUALITY_DEST:-/var/www/mtl-aq/}"

cd "$(dirname "$0")/.."

# Make sure cargo / trunk are on PATH when invoked from a non-login shell.
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

mode="code"
case "${1:-}" in
    --download)
        scripts/fetch-archive.sh
        mode="data"
        ;;
    --data)
        mode="data"
        ;;
    "")
        ;;
    *)
        echo "Unknown option: $1 (use --data or --download)" >&2
        exit 1
        ;;
esac

# Regenerate the compact data only when a data deploy is requested.
if [ "$mode" = "data" ]; then
    if ls data-src/rsqa-multi-polluants*.csv >/dev/null 2>&1; then
        python3 scripts/preprocess.py
    else
        echo "No raw inputs in data-src/ — skipping preprocess (using existing static/data/)."
    fi
fi

trunk build --release

if [ "$mode" = "data" ]; then
    # Content-based compare (--checksum): preprocess rewrites every file each run,
    # so mtimes always differ; comparing by checksum means only files whose bytes
    # actually changed (e.g. a newly added year + map-stats) are transferred.
    rsync -av --delete --checksum dist/ "${REMOTE}:${DEST}"
else
    # Code-only: don't read or transfer the large data tiers already on the server.
    # rsync --delete leaves excluded paths on the receiver intact, so the server's
    # data is preserved while stale code bundles (old hashed wasm/js) are cleaned up.
    rsync -av --delete \
        --exclude='/data/series/' \
        --exclude='/data/series-daily/' \
        dist/ "${REMOTE}:${DEST}"
fi
