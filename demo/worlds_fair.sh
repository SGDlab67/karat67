#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
echo "=== Pass: well-formed UserMetadata ==="
cargo run --quiet --bin karat -- shape --account-type UserMetadata --len 1032
echo "=== Fail: empty payload (silent corruption class) ==="
if cargo run --quiet --bin karat -- shape --account-type UserMetadata --len 0; then
  echo "EXPECTED FAIL but got exit 0" >&2
  exit 1
fi
echo "ACCEPTANCE OK: empty payload failed under green shape check"
