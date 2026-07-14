#!/bin/bash
set -euo pipefail
# shellcheck source=_lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/_lib.sh"

export CARGO_NET_OFFLINE=false
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$target_root/test}"
mkdir -p "$CARGO_TARGET_DIR"

# Each crate is its own cargo workspace in this repo, so nextest's default
# `<workspace>/.config/nextest.toml` lookup would miss our shared config at
# repo root. Pass it explicitly.
nextest_cfg="$repo_root/.config/nextest.toml"
nx=(nextest run --config-file "$nextest_cfg" --profile ci)

log_section "Running tests"
test_scope="${RMK_TEST_SCOPE:-all}"
if [[ "$test_scope" != "all" && "$test_scope" != "crates" && "$test_scope" != "rmk" ]]; then
    printf 'Invalid RMK_TEST_SCOPE: %s\n' "$test_scope" >&2
    exit 2
fi

if [[ "$test_scope" != "rmk" ]]; then
    cargo +stable "${nx[@]}" --manifest-path rmk-config/Cargo.toml
    cargo +stable "${nx[@]}" --manifest-path rmk-types/Cargo.toml
    # Exercise the rynk protocol module (gated behind `rynk`).
    cargo +stable "${nx[@]}" --manifest-path rmk-types/Cargo.toml --features host
    cargo +stable "${nx[@]}" --manifest-path rmk-types/Cargo.toml --features steno
    cargo +stable "${nx[@]}" --manifest-path rmk-macro/Cargo.toml
fi

if [[ "$test_scope" != "crates" ]]; then
    shard_index="${RMK_TEST_SHARD_INDEX:-0}"
    shard_count="${RMK_TEST_SHARD_COUNT:-1}"
    if ! [[ "$shard_index" =~ ^[0-9]+$ && "$shard_count" =~ ^[1-9][0-9]*$ ]] || (( shard_index >= shard_count )); then
        printf 'Invalid RMK test shard: index=%s count=%s\n' "$shard_index" "$shard_count" >&2
        exit 2
    fi

    for i in "${!RMK_FEATURESETS[@]}"; do
        (( i % shard_count == shard_index )) || continue
        feats="${RMK_FEATURESETS[$i]}"
        if [[ -z "$feats" ]]; then
            cargo +stable "${nx[@]}" --manifest-path rmk/Cargo.toml --no-default-features
        else
            cargo +stable "${nx[@]}" --manifest-path rmk/Cargo.toml --no-default-features --features "$feats"
        fi
    done
fi

# Doctests: nextest does not run them. rmk/ and rmk-macro/ have `doctest = false`,
# so only rmk-types and rmk-config need a separate --doc pass.
if [[ "$test_scope" != "rmk" ]]; then
    log_section "Running doctests"
    cargo +stable test --manifest-path rmk-types/Cargo.toml --doc
    cargo +stable test --manifest-path rmk-types/Cargo.toml --features host --doc
    cargo +stable test --manifest-path rmk-config/Cargo.toml --doc
fi
