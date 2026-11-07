#!/bin/sh
# Rebuilds sim-wasm from the checked-out tree (build-wasm.sh, fixed build stamp) and compares it with the
# checked-in lab binary. The build is deterministic for one wasm-pack/rustc toolchain, so a mismatch on the
# toolchain that produced the checked-in binary means the lab is stale; another toolchain may differ.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
"$root/tools/lab/build-wasm.sh" "$out"
for f in sim_wasm.js sim_wasm_bg.wasm; do
    cmp -s "$out/$f" "$root/tools/lab/$f" || { echo "stale: tools/lab/$f differs from a fresh build" >&2; exit 1; }
done
echo "tools/lab matches a fresh build of crates/sim-wasm"
