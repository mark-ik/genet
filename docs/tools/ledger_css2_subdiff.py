"""F3b: slice a reftest directory's S-only files into workable buckets.

The reftest ledger reports each directory as one number (CSS2: 449 files
Stylo renders and Livery does not). That is a cluster, not a work item, and
its parts fail for unrelated reasons. This groups those files so the cluster
can be sliced.

Written for F3b cluster 2 (CSS2) and general since: point `LEDGER_DIR` at any
directory in the ledger. Nested corpora (CSS2) bucket by subdirectory; flat
ones (css-flexbox, css-grid) bucket by test-name family, which is how WPT
names variants of one feature.

Reads the same expectation JSON `ledger_reftest_diff.py` does, so it needs no
new run once `css_CSS2_stylo.json` and `css_CSS2_livery.json` exist:

    LEDGER_OUT=<ledger dir> python docs/tools/ledger_css2_subdiff.py
    LEDGER_OUT=<ledger dir> LEDGER_DIR=css_css-flexbox python ...

Reports L-only alongside S-only on purpose. A bucket where both are large is
churn (the engines disagree about the same feature in both directions) and is
usually one fidelity bug; a bucket where S-only is large and L-only is ~0 is
a straight capability gap. Those want different work, and the net delta the
F3b table leads with hides the difference.
"""
import json, os, glob, collections

SP = os.environ.get("LEDGER_OUT", "./target/ledger-reftest")
DIR = os.environ.get("LEDGER_DIR", "css_CSS2")
# Where the WPT corpus lives, for the weighting column. Optional.
TESTS_ROOT = os.environ.get("WPT_TESTS_ROOT", "tests/wpt/tests")

def status(v):
    # reftest expectations store a bare status string; testharness stores a dict
    return v if isinstance(v, str) else v.get("status")


def load(p):
    try:
        return json.load(open(p, encoding="utf-8")).get("tests", {})
    except FileNotFoundError:
        return None
    except Exception as e:
        print(f"  ! {p}: {e}")
        return None


# The directory name inside the ledger slug, e.g. css_css-flexbox -> css-flexbox.
CORPUS = DIR.split("_", 1)[1] if "_" in DIR else DIR


def stem_family(name):
    """`flex-basis-011.html` -> `flex-basis`.

    WPT names variants of one feature by suffixing an index, sometimes with a
    trailing letter (`fixed-table-layout-003b`). Stripping that suffix groups
    a feature's variants, which is the unit worth slicing in a flat corpus.
    """
    stem = name.rsplit(".", 1)[0]
    parts = stem.split("-")
    while len(parts) > 1 and parts[-1].rstrip("abcdefghijklmnopqrstuvwxyz").isdigit():
        parts.pop()
    return "-".join(parts) if parts else stem


def bucket(url):
    """Map a test URL to a bucket: its subdirectory, else its name family."""
    parts = [p for p in url.split("/") if p]
    if CORPUS in parts:
        parts = parts[parts.index(CORPUS) + 1:]
    if not parts:
        return "(unknown)"
    if len(parts) > 1:
        return parts[0]
    return stem_family(parts[0])


def corpus_sizes():
    """File counts per bucket, for weighting. Empty if the corpus is absent."""
    root = os.path.join(TESTS_ROOT, "css", CORPUS)
    if not os.path.isdir(root):
        return {}
    sizes = {}
    for entry in sorted(os.listdir(root)):
        full = os.path.join(root, entry)
        if not os.path.isdir(full):
            continue
        n = 0
        for _, _, files in os.walk(full):
            n += sum(1 for f in files if f.endswith((".html", ".htm", ".xht", ".xhtml")))
        if n:
            sizes[entry] = n
    return sizes


def main():
    s = load(f"{SP}/{DIR}_stylo.json")
    l = load(f"{SP}/{DIR}_livery.json")
    if s is None or l is None:
        print(f"no ledger data at {SP}/{DIR}_{{stylo,livery}}.json")
        print("Run docs/tools/ledger_reftest.sh first (GPU, sequential, slow).")
        print("Write the output somewhere `cargo clean` cannot reach: these")
        print("JSON files are the expensive artifact, not the diff.")
        raise SystemExit(1)

    s_only = collections.Counter()
    l_only = collections.Counter()
    agreed = collections.Counter()
    s_only_files = collections.defaultdict(list)
    for url, sv in s.items():
        if url not in l:
            continue
        sp, lp = status(sv) == "pass", status(l[url]) == "pass"
        b = bucket(url)
        if sp and not lp:
            s_only[b] += 1
            s_only_files[b].append(url)
        elif lp and not sp:
            l_only[b] += 1
        elif sp and lp:
            agreed[b] += 1

    total_s = sum(s_only.values())
    sizes = corpus_sizes()

    print(f"{CORPUS} sub-diff: {total_s} S-only files across {len(s_only)} buckets")
    print(f"  (L-only {sum(l_only.values())}, both-pass {sum(agreed.values())})\n")
    head = f"{'bucket':<26}{'S-only':>8}{'share':>8}{'L-only':>8}{'both':>8}{'corpus':>8}"
    print(head)
    print("-" * len(head))
    for b, n in s_only.most_common():
        share = f"{100.0 * n / total_s:.1f}%" if total_s else "-"
        corpus = str(sizes.get(b, "")) if sizes else ""
        print(f"{b:<26}{n:>8}{share:>8}{l_only[b]:>8}{agreed[b]:>8}{corpus:>8}")
    print("-" * len(head))
    print(f"{'TOTAL':<26}{total_s:>8}")

    # The slicing hint. A bucket with S-only high and L-only near zero is a
    # capability gap; one with both high is bidirectional churn.
    print("\n=== READING ===")
    for b, n in s_only.most_common():
        if n < max(5, total_s // 50):
            continue
        other = l_only[b]
        kind = "capability gap" if other <= n // 4 else "bidirectional churn"
        print(f"  {b:<24} {n:>4} S-only / {other:>4} L-only  -> {kind}")

    out = os.environ.get("SUBDIFF_OUT")
    if out:
        with open(out, "w", encoding="utf-8") as fh:
            json.dump(
                {b: sorted(v) for b, v in sorted(s_only_files.items())},
                fh,
                indent=1,
            )
        print(f"\nper-bucket S-only file lists written to {out}")
    else:
        print("\nSet SUBDIFF_OUT=<path> to dump the per-bucket file lists.")


if __name__ == "__main__":
    main()
