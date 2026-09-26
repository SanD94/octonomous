#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

cargo run \
  --quiet \
  --manifest-path "$repo_root/tools/codegen/Cargo.toml" \
  -- \
  "$repo_root/docs/openapi-v2.0.18.json" \
  "$repo_root/crates/octonomous-core/src/generated/mod.rs"

cargo fmt --manifest-path "$repo_root/Cargo.toml" --all
