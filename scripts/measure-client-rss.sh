#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
cargo build --release -p octonomous-core --example memory >/dev/null
runs=${RUNS:-5}
printf 'run\tstate\trss_kib\trss_mib\n'
i=1
while [ "$i" -le "$runs" ]; do
    target/release/examples/memory | sed "s/^/$i\t/"
    i=$((i + 1))
done
