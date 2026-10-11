"""Catalog every account discriminator in Carbon's decoders.

usage: python3 -I evidence/carbon_discriminators.py <carbon repo> <out csv>
Prints a summary; writes decoder,program_id,account_type,discriminator,sha_ok.
"""
import collections, csv, hashlib, re, sys
from pathlib import Path

root, out = Path(sys.argv[1]), sys.argv[2]
rows, empty = [], []
for dec in sorted((root / "decoders").iterdir()):
    acc = dec / "src/accounts"
    lib = dec / "src/lib.rs"
    pid = re.search(r'from_str_const\("(\w+)"\)', lib.read_text()) if lib.exists() else None
    found = 0
    for f in sorted(acc.glob("*.rs")) if acc.is_dir() else []:
        src = f.read_text()
        d = re.search(r"discriminator != \[([\d,\s]+)\]", src)
        n = re.search(r"pub struct (\w+)", src)
        if not (d and n):
            continue
        disc = bytes(int(x) for x in d.group(1).split(",") if x.strip())
        sha = hashlib.sha256(f"account:{n.group(1)}".encode()).digest()[:8]
        rows.append(dict(decoder=dec.name, program_id=pid.group(1) if pid else "",
                         account_type=n.group(1), discriminator=disc.hex(), sha_ok=disc == sha))
        found += 1
    if not found:
        empty.append(dec.name)

with open(out, "w", newline="") as fh:
    w = csv.DictWriter(fh, fieldnames=list(rows[0]))
    w.writeheader()
    w.writerows(rows)

by_disc = collections.defaultdict(list)
for r in rows:
    by_disc[r["discriminator"]].append(r)
shared = {d: rs for d, rs in by_disc.items() if len({r["program_id"] for r in rs}) > 1}
print(f"decoders: {len(list((root / 'decoders').iterdir()))}, with account discriminators: "
      f"{len({r['decoder'] for r in rows})}, account types: {len(rows)}")
print(f"sha256('account:<Name>') matches: {sum(r['sha_ok'] for r in rows)} / {len(rows)}")
print(f"discriminators shared by more than one program: {len(shared)}, "
      f"covering {sum(len(v) for v in shared.values())} account types")
for d, rs in sorted(shared.items(), key=lambda kv: -len(kv[1])):
    print(f"  {rs[0]['account_type']:<24} x{len(rs)}: {', '.join(sorted(r['decoder'].removesuffix('-decoder') for r in rs))}")
print("no account discriminators found in:", ", ".join(e.removesuffix("-decoder") for e in empty))
print("sha mismatches:", [(r["decoder"], r["account_type"]) for r in rows if not r["sha_ok"]])
