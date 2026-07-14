#!/bin/bash
set -euo pipefail
# shellcheck source=_lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/_lib.sh"

# The host tooling is its own cargo workspace.
cd "$repo_root/rynk"

export CARGO_TERM_COLOR=always
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$target_root/host}"
mkdir -p "$CARGO_TARGET_DIR"

log_section "Tests"
cargo +stable test --workspace --lib --tests

log_section "Doctests"
cargo +stable test -p rynk --doc

log_section "Wasm smoke check"
cargo +stable check -p rynk --lib --target wasm32-unknown-unknown
cargo +stable check -p rynk-wasm --target wasm32-unknown-unknown

log_section "Wasm package build"
# wasm-pack emits the JS package plus generated .d.ts files under rynk-wasm/pkg/.
# The generated package is intentionally ignored rather than checked in.
(cd rynk-wasm && wasm-pack build --target web >/dev/null)
npx --yes --package typescript@5.9.3 tsc \
    --noEmit --strict --target ES2022 --lib ES2022,DOM,ESNext.Disposable \
    --module ES2022 --moduleResolution bundler \
    rynk-wasm/wasm-smoke.ts

log_section "Clippy"
cargo +stable clippy --workspace --lib --tests --examples -- -D warnings
cargo +stable clippy -p rynk-wasm --target wasm32-unknown-unknown -- -D warnings
