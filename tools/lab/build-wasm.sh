#!/bin/sh
# Builds sim-wasm and copies sim_wasm.js / sim_wasm_bg.wasm into the given directory (default: tools/lab).
# SIM_BUILD_COMMIT_OVERRIDE / SIM_BUILD_DIRTY_OVERRIDE replace the git stamp in the binary with a fixed one,
# so the checked-in lab build is reproducible from its own sources; native builds still stamp the real commit.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
dest=${1:-$root/tools/lab}
pkg=$(mktemp -d)
trap 'rm -rf "$pkg"' EXIT
SIM_BUILD_COMMIT_OVERRIDE=lab-build SIM_BUILD_DIRTY_OVERRIDE=false \
    wasm-pack build "$root/crates/sim-wasm" --target no-modules --release --out-dir "$pkg" >/dev/null 2>&1
cp "$pkg/sim_wasm.js" "$pkg/sim_wasm_bg.wasm" "$dest/"
