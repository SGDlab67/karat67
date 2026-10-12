#!/usr/bin/env bash
# Step-through World's Fair demo: silent prep and preflight, then Enter per beat.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v karat >/dev/null || { echo "karat not on PATH: run cargo install --path . --locked" >&2; exit 1; }
export KARAT_RPC_URL="${KARAT_RPC_URL:-https://api.mainnet-beta.solana.com}"

rpc() { curl -sf "$KARAT_RPC_URL" -H 'content-type: application/json' -d "$1"; }
die() { echo "prep failed: $1" >&2; exit 1; }

# A dormant Obligation (no transaction in years), the same one the site's
# Kamino preset uses. An active one changes on chain between preflight and the
# Pass beat, which then fails on camera.
PK=2AHc6PtJftXmfbNvkvPfpuWzV7LvRTJCutzWqgMJKVpq
GOOD=$(rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$PK\",{\"encoding\":\"base64\"}]}" |
  python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["value"]["data"][0])' 2>/dev/null) || die "getAccountInfo $PK"
BAD=$(python3 -c 'import base64,sys;b=bytearray(base64.b64decode(sys.argv[1]));b[100]^=0xFF;print(base64.b64encode(b).decode())' "$GOOD")
END=$(rpc '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[{"commitment":"finalized"}]}' |
  python3 -c 'import json,sys;print(json.load(sys.stdin)["result"])' 2>/dev/null) || die "getSlot"
START=$((END - 100))
# Drop three produced slots so completeness has something to find.
SLOTS=$(rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBlocks\",\"params\":[$START,$END]}" |
  python3 -c 'import json,sys;b=json.load(sys.stdin)["result"];assert len(b)>43;print(",".join(map(str,b[:40]+b[43:])))' 2>/dev/null) || die "getBlocks $START..$END"

b1=(karat account --account "$PK" --indexed-base64 "")
b2=(karat account --account "$PK" --indexed-base64 "$GOOD")
b3=(karat account --account "$PK" --indexed-base64 "$BAD")
b4=(karat completeness --start "$START" --end "$END" --slots "$SLOTS")

# pre <label> pass|fail <cmd...>: a fail beat must exit non-zero with a real
# Fail, so an RPC hiccup (Skipped) is caught here instead of on camera.
pre() {
  local label=$1 want=$2 out code=0; shift 2
  out=$("$@" 2>&1) || code=$?
  if [[ $want == pass && $code -eq 0 ]] || [[ $want == fail && $code -ne 0 && $out == *'"status": "Fail"'* ]]; then return; fi
  echo "preflight failed: $label (exit $code)" >&2; exit 1
}
pre "beat 1, empty payload" fail "${b1[@]}"
pre "beat 2, good payload" pass "${b2[@]}"
pre "beat 3, bad payload" fail "${b3[@]}"
pre "beat 4, completeness" fail "${b4[@]}"

echo "preflight OK, press Enter to start"
read -rs || true
clear 2>/dev/null || true

# beat <shown> <cmd...>: wait for Enter, show the command, run it, show exit.
beat() {
  local shown=$1 code=0; shift
  read -rs || true
  printf '\033[1m$ %s\033[0m\n' "$shown"
  "$@" || code=$?
  if ((code == 0)); then printf '\033[32mexit 0\033[0m\n\n'; else printf '\033[31mexit %d\033[0m\n\n' "$code"; fi
}
beat 'karat account --account $PK --indexed-base64 ""' "${b1[@]}"
beat 'karat account --account $PK --indexed-base64 "$GOOD"' "${b2[@]}"
beat 'karat account --account $PK --indexed-base64 "$BAD"' "${b3[@]}"
beat 'karat completeness --start $START --end $END --slots "$SLOTS"' "${b4[@]}"

echo "github.com/SGDlab67/karat67"
