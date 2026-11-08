#!/bin/sh
# Builds sim-wasm and copies sim_wasm.js / sim_wasm_bg.wasm into the given directory (default: tools/lab).
# SIM_BUILD_COMMIT_OVERRIDE / SIM_BUILD_DIRTY_OVERRIDE replace the git stamp in the binary with a fixed one,
# so the checked-in lab build is reproducible from its own sources; native builds still stamp the real commit.
# Panic locations embed source paths: the checkout, the cargo registry and the toolchain's std sources
# (rustc points std at the local rust-src copy when one is installed). Remapping all three to fixed
# prefixes makes the binary independent of where the repository, cargo and rustup live. A caller's
# RUSTFLAGS are kept and the remaps appended; any other flag changes the binary, so check-wasm.sh
# compares only against a build made without extra flags.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
dest=${1:-$root/tools/lab}
pkg=$(mktemp -d)
trap 'rm -rf "$pkg"' EXIT
cargo_home=${CARGO_HOME:-$HOME/.cargo}
sysroot=$(rustc --print sysroot)
RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$root=/lab --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$sysroot=/rustc-sysroot" \
SIM_BUILD_COMMIT_OVERRIDE=lab-build SIM_BUILD_DIRTY_OVERRIDE=false \
    wasm-pack build "$root/crates/sim-wasm" --target no-modules --release --out-dir "$pkg" >/dev/null 2>&1
cp "$pkg/sim_wasm.js" "$pkg/sim_wasm_bg.wasm" "$dest/"
