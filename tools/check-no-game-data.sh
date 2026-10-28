#!/usr/bin/env bash
# Inspect the staged blobs (--staged) or all indexed files (--tracked).
set -euo pipefail
exec python3 "$(dirname "$(realpath "$0")")/check-no-game-data.py" "${1:---staged}"
