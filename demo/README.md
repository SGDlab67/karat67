# World's Fair demo runner

```bash
cargo install --path . --locked   # once, puts `karat` on PATH
bash demo/worlds_fair.sh
```

The script fetches a live Kamino Obligation and a fresh slot window from
mainnet, builds a corrupted copy, and runs every beat once off screen. If any
beat would not produce its expected result, it stops before the camera part.

Then it clears the screen. Press Enter to advance each beat:

1. `karat account` on an empty payload: shape Fail, no RPC call.
2. `karat account` on the live bytes: Pass, exit 0.
3. `karat account` on the live bytes with one byte flipped: reconcile Fail at offset 100.
4. `karat completeness` with three produced slots dropped: Fail, 3 missing.

`KARAT_RPC_URL` defaults to the public mainnet endpoint and is never printed.
