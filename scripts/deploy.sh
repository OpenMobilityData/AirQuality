#!/usr/bin/env bash
# Build and deploy AirQualityMTL to the production VPS.
#
# Served at https://mtl-aq.org — hosted on the same server as bikestat.org
# under its own vhost. Override the destination with AIRQUALITY_REMOTE /
# AIRQUALITY_DEST.
#
# Modes:
#   ./scripts/deploy.sh             Code + committed daily tier: build + rsync the
#                                   app AND the committed daily series (the Map and
#                                   Time-series both read it, so it must track the
#                                   code). Does NOT re-run preprocess or re-send the
#                                   git-ignored ~250 MB hourly tier (only the Map's
#                                   time-of-day filter needs that; deploy it once
#                                   with --data, then it's stable).
#   ./scripts/deploy.sh --data      Also regenerate static/data from data-src/ and
#                                   sync every tier incl. the hourly series
#                                   (content-compared, so unchanged files aren't
#                                   re-sent). Use after adding a year or stations.
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
    # Content-based compare (--checksum): preprocess (and a git checkout) rewrites
    # files with fresh mtimes, so comparing by checksum means only files whose bytes
    # actually changed (e.g. a newly added year) are transferred.
    # Exclude /.well-known/ so --delete leaves the certbot ACME challenge dir (owned
    # by the cert renewal, not us) intact instead of failing to remove it.
    rsync -av --delete --checksum \
        --exclude='/.well-known/' \
        dist/ "${REMOTE}:${DEST}"
else
    # Code + daily tier: sync everything except the heavy git-ignored hourly series
    # (~250 MB), which is format-stable and deployed separately via --data. The
    # daily tier IS synced — the Map/Series read it and it must match the code; it's
    # small (~27 MB) and --checksum avoids resending bytes that didn't change.
    # rsync --delete leaves the excluded /data/series/ on the receiver intact while
    # cleaning stale code bundles (old hashed wasm/js). /.well-known/ is likewise
    # excluded so --delete leaves the certbot ACME challenge dir (owned by the cert
    # renewal, not us) intact instead of failing to remove it.
    rsync -av --delete --checksum \
        --exclude='/data/series/' \
        --exclude='/.well-known/' \
        dist/ "${REMOTE}:${DEST}"
fi
