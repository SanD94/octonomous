#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
dependencies=$(cargo tree -p octonomous-core --edges normal --prefix none --format '{p}')
if printf '%s\n' "$dependencies" | grep -Eq '^(ratatui|crossterm) v'; then
    printf '%s\n' 'octonomous-core must not depend on ratatui or crossterm' >&2
    exit 1
fi
printf '%s\n' 'octonomous-core terminal dependency check passed'
