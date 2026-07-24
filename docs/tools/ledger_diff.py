"""F3 fidelity ledger: diff Stylo vs Livery pass sets per WPT directory."""
import json, os, glob, collections

SP = os.environ.get("LEDGER_OUT", "./target/ledger")

def load(p):
    try:
        return json.load(open(p, encoding="utf-8")).get("tests", {})
    except Exception:
        return None

dirs = sorted({os.path.basename(f).rsplit("_", 1)[0]
               for f in glob.glob(SP + "/*_stylo.json")})

rows, regressions = [], collections.OrderedDict()
tot_s = tot_l = 0
for d in dirs:
    s = load(f"{SP}/{d}_stylo.json")
    l = load(f"{SP}/{d}_livery.json")
    if s is None or l is None:
        continue
    # subtest totals
    ss = sum(v.get("subtests_passed", 0) for v in s.values())
    ls = sum(v.get("subtests_passed", 0) for v in l.values())
    tot_s += ss; tot_l += ls
    # per-file: where stylo strictly beats livery
    worse = []
    for url, sv in s.items():
        lv = l.get(url)
        if lv is None:
            continue
        sp, lp = sv.get("subtests_passed", 0), lv.get("subtests_passed", 0)
        s_ok, l_ok = sv.get("status") == "pass", lv.get("status") == "pass"
        if (s_ok and not l_ok) or (sp > lp):
            worse.append((url, sp, lp, sv.get("subtests_total", 0)))
    rows.append((d, ss, ls, len(worse), len(s)))
    if worse:
        regressions[d] = sorted(worse, key=lambda x: -(x[1] - x[2]))

print(f"{'directory':<28}{'stylo':>8}{'livery':>8}{'delta':>8}{'S>L files':>11}")
print("-" * 63)
for d, ss, ls, w, n in sorted(rows, key=lambda r: (r[3] == 0, -(r[3]))):
    print(f"{d:<28}{ss:>8}{ls:>8}{ls-ss:>+8}{w:>11}")
print("-" * 63)
print(f"{'TOTAL':<28}{tot_s:>8}{tot_l:>8}{tot_l-tot_s:>+8}")

print("\n\n=== CLUSTERS WHERE STYLO BEATS LIVERY (F3 slice candidates) ===")
if not regressions:
    print("  none")
for d, items in regressions.items():
    gap = sum(a - b for _, a, b, _ in items)
    print(f"\n-- {d}: {len(items)} files, {gap} subtests behind")
    for url, sp, lp, st in items[:8]:
        print(f"     {sp:>4} -> {lp:<4} (of {st:<4}) {url.split('/', 2)[-1]}")
    if len(items) > 8:
        print(f"     ... +{len(items)-8} more files")
