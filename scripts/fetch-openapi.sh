#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
committed="$root/docs/openapi-v2.0.18.json"
server_url=${1:-${OPENCODE_SERVER:-}}

if [ -z "$server_url" ]; then
    server_url=$(opencode service status)
fi

config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
password_file="$config_home/opencode/service.json"

if [ ! -f "$password_file" ]; then
    echo "error: OpenCode service credentials not found at $password_file" >&2
    exit 1
fi

password=$(jq -er '.password' "$password_file")
tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

curl --fail --silent --show-error \
    --user "opencode:$password" \
    "${server_url%/}/openapi.json" > "$tmp_dir/fetched.json"

jq --sort-keys . "$committed" > "$tmp_dir/committed.normalized.json"
jq --sort-keys . "$tmp_dir/fetched.json" > "$tmp_dir/fetched.normalized.json"

diff -u \
    --label "docs/openapi-v2.0.18.json (committed)" \
    --label "${server_url%/}/openapi.json (fetched)" \
    "$tmp_dir/committed.normalized.json" \
    "$tmp_dir/fetched.normalized.json"
