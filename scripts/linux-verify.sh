#!/bin/sh
# Runs the Linux-only checks exactly as the native CI lane does, so a failure
# that the Windows authoring host cannot observe is reproduced here.
#
# This script exists because `cargo clippy` and `cargo test` on Windows cannot
# see defects in `#[cfg(unix)]` code: an unused import, an unused `mut`, and a
# clippy lint that only fires where Unix permission code compiles were all
# invisible locally while failing every CI run.
set -eu

apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq build-essential pkg-config nodejs npm >/dev/null 2>&1

echo "=== rustc / node ==="
rustc --version
cargo --version
node --version

echo "=== fmt --check ==="
cargo fmt --all --check
echo "fmt: OK"

echo "=== clippy (warnings denied) ==="
cargo clippy --workspace --all-targets --all-features -- -D warnings >/tmp/clippy.log 2>&1
echo "clippy: OK"

echo "=== workspace tests (CI RUSTFLAGS) ==="
RUSTFLAGS="-D warnings" cargo test --workspace --all-features >/tmp/test.log 2>&1
echo "tests: OK"
grep -c "test result: ok" /tmp/test.log

echo "=== unix permission and storage tests (run only on non-Windows) ==="
cargo test -p jarvis-infrastructure --all-features "paths::" "storage::" 2>&1 | tail -6

echo "=== build release binaries ==="
cargo build --release -p jarvisd -p jarvis-cli >/tmp/build.log 2>&1
echo "build: OK"

echo "=== clean-machine journey ==="
node scripts/clean-machine-smoke.mjs target/release 2>&1 | tail -4

echo "=== release verification journey ==="
JARVIS_TARGET=x86_64-unknown-linux-gnu node scripts/release-verify-smoke.mjs target/release 2>&1 | tail -4

echo "=== install journey ==="
JARVIS_TARGET=x86_64-unknown-linux-gnu node scripts/install-smoke.mjs target/release 2>&1 | tail -4

echo "ALL LINUX CHECKS DONE"
