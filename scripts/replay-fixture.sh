#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture="$root/docs/fixtures/prompt-basic-v2.0.18.sse"

# Emit each SSE data payload as one validated, compact JSON value. SSE comments
# (including heartbeats) and blank frame separators are intentionally ignored.
sed -n 's/^data: //p' "$fixture" | jq --compact-output .
