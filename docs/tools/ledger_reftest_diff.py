"""F3b reftest ledger: diff Stylo vs Livery layout/paint pass sets per directory."""
import json, os, glob

SP = os.environ.get("LEDGER_OUT", "./target/ledger-reftest")

def status(v):
    # reftest expectations store a bare status string; testharness stores a dict
    return v if isinstance(v, str) else v.get("status")

def load(p):
    try:
        return json.load(open(p, encoding="utf-8")).get("tests", {})
    except Exception:
        return None

dirs = sorted({os.path.basename(f).rsplit("_", 1)[0]
               for f in glob.glob(SP + "/*_stylo.json")})

rows, regressions = [], {}
tot_s = tot_l = 0
for d in dirs:
    s, l = load(f"{SP}/{d}_stylo.json"), load(f"{SP}/{d}_livery.json")
    if s is None or l is None:
        continue
    sp = sum(1 for v in s.values() if status(v) == "pass")
    lp = sum(1 for v in l.values() if status(v) == "pass")
    tot_s += sp; tot_l += lp
    # files stylo renders correctly and livery does not
    worse = [u for u, sv in s.items()
             if status(sv) == "pass" and u in l and status(l[u]) != "pass"]
    rows.append((d, sp, lp, len(worse), len(s)))
    if worse:
        regressions[d] = worse

print(f"{'directory':<26}{'stylo':>8}{'livery':>8}{'delta':>8}{'S>L files':>11}{'files':>8}")
print("-" * 69)
for d, sp, lp, w, n in sorted(rows, key=lambda r: r[2] - r[1]):
    print(f"{d:<26}{sp:>8}{lp:>8}{lp-sp:>+8}{w:>11}{n:>8}")
print("-" * 69)
print(f"{'TOTAL':<26}{tot_s:>8}{tot_l:>8}{tot_l-tot_s:>+8}")

print("\n=== FILES STYLO RENDERS AND LIVERY DOES NOT (F3b slice candidates) ===")
if not regressions:
    print("  none")
for d, items in regressions.items():
    print(f"\n-- {d}: {len(items)} files")
    for u in items[:10]:
        print(f"     {u.split('/', 2)[-1]}")
    if len(items) > 10:
        print(f"     ... +{len(items)-10} more")
