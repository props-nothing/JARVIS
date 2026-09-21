#!/bin/sh
# Diagnoses the Linux failures the native CI lane would hit.
set -eu

apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq build-essential pkg-config nodejs npm >/dev/null 2>&1

echo "=== 1. Does cargo test accept two positional filters? ==="
cargo test -p jarvis-infrastructure --all-features paths:: storage:: 2>&1 | tail -4 || true

echo "=== 2. Does it accept them after -- ? ==="
cargo test -p jarvis-infrastructure --all-features -- paths:: storage:: 2>&1 | grep -E "test result|running|error" | head -8 || true

echo "=== 3. Clean-machine journey, full output ==="
cargo build --release -p jarvisd -p jarvis-cli >/dev/null 2>&1
node scripts/clean-machine-smoke.mjs target/release 2>&1 | tail -40 || true
