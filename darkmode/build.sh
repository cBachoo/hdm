#!/usr/bin/env bash
# Builds both variants into dist/ (the default rules from tools/overrides.json are embedded):
#   dist/release/hachimi_darkmode.dll   user build (simple menu, no diagnostics)
#   dist/dev/hachimi_darkmode.dll       dev build (asset logging, dumps, debug tools)
set -euo pipefail
cd "$(dirname "$0")"

build() {
    local name="$1"; shift
    cargo build --release "$@"
    rm -rf "dist/$name"
    mkdir -p "dist/$name"
    cp target/release/hachimi_darkmode.dll target/release/hachimi_darkmode.pdb "dist/$name/"
    echo "built dist/$name"
}

build dev --features dev
build release
