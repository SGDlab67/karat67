#!/usr/bin/env bash
# Camera-friendly 60s World's Fair demo — one command, clear banners.
set -euo pipefail
cd "$(dirname "$0")/.."

banner() {
  printf '\n'
  printf '══════════════════════════════════════════════════════════\n'
  printf '  %s\n' "$1"
  printf '══════════════════════════════════════════════════════════\n'
}

banner "karat67 · World's Fair demo"
echo "  Chain-truth shape gate — not a liveness dashboard"
echo "  Account: UserMetadata (Kamino Lend)"

banner "PASS · well-formed UserMetadata · len 1032"
cargo run --quiet --bin karat -- --json shape --account-type UserMetadata --len 1032
echo "  → exit 0  shape(UserMetadata) Pass"

banner "FAIL · empty payload · silent corruption class · len 0"
set +e
cargo run --quiet --bin karat -- --json shape --account-type UserMetadata --len 0
fail_status=$?
set -e
if [[ "$fail_status" -eq 0 ]]; then
  echo "EXPECTED FAIL but got exit 0" >&2
  exit 1
fi
echo "  → exit $fail_status  shape Fail (empty payload)"

banner "ACCEPTANCE OK"
echo "  Empty payload failed under a green shape check."
echo "  Liveness can stay green. Shape cannot."
echo
