//! F0's receipt: the consumed-longhand contract, asserted.
//!
//! The cutover plan's F0 stage closes the gap between Livery's catalog and the
//! 126 longhands the current Genet product path actually reads
//! (`docs/2026-07-13_genet_consumed_css_property_audit.md`). Its receipt is
//! "the census reads 126/126 consumed implemented".
//!
//! That receipt lives here rather than only in `tools/import-stylo-db` on
//! purpose. The tool needs a stylo fork checkout to run at all, and the fork
//! archives at F5 of the cutover plan; a receipt that needs it would die with
//! it. This test reads two checked-in files and nothing else, so it keeps
//! working after the fork is gone.

use std::collections::BTreeSet;

use livery::{PropertyId, ShorthandId};

const CONSUMED: &str = include_str!("../consumed_longhands.toml");

struct Consumed {
    name: String,
    consumers: String,
}

/// The audit's overlap table, as data. Parsed with the same `toml` the build
/// script uses, so the file cannot drift into a shape the generator rejects.
fn consumed_set() -> Vec<Consumed> {
    let table: toml::Table = CONSUMED.parse().expect("consumed_longhands.toml parses");
    let longhands = table
        .get("longhands")
        .and_then(|value| value.as_table())
        .expect("consumed_longhands.toml has a [longhands] table");
    let mut entries: Vec<Consumed> = longhands
        .values()
        .map(|entry| {
            let entry = entry.as_table().expect("each entry is a table");
            Consumed {
                name: entry
                    .get("name")
                    .and_then(|value| value.as_str())
                    .expect("entry has a name")
                    .to_owned(),
                consumers: entry
                    .get("consumers")
                    .and_then(|value| value.as_str())
                    .expect("entry has consumers")
                    .to_owned(),
            }
        })
        .collect();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// A consumed name is covered if the parser resolves it, as a longhand or as
/// the other kind. Livery models a few upstream shorthands as single longhands
/// (`background-position`), and the reverse; the catalog census treats those as
/// covered, so this receipt must agree with it.
fn is_covered(name: &str) -> bool {
    PropertyId::from_css_name(name).is_some() || ShorthandId::from_css_name(name).is_some()
}

#[test]
fn the_consumed_set_is_the_audits_126_longhands() {
    let consumed = consumed_set();
    let declared = {
        let table: toml::Table = CONSUMED.parse().expect("parses");
        table["meta"]["count"]
            .as_integer()
            .expect("meta.count is an integer") as usize
    };
    assert_eq!(
        consumed.len(),
        declared,
        "consumed_longhands.toml lists {} names but meta.count says {declared}",
        consumed.len(),
    );
    assert_eq!(
        consumed.len(),
        126,
        "the audit's contract is 126 longhands; re-run the audit before changing this",
    );
    let unique: BTreeSet<&str> = consumed.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(unique.len(), consumed.len(), "duplicate consumed names");
    for entry in &consumed {
        assert!(
            entry.consumers.chars().all(|ch| "LRCA ".contains(ch)),
            "{}: unknown consumer surface {:?}",
            entry.name,
            entry.consumers,
        );
    }
}

/// How many consumed longhands Livery may still be missing.
///
/// F0's receipt is this reaching 0; until then it is a ratchet. A permanently
/// red test would just be noise everyone learns to skip, and it would block the
/// workspace-green rule the cutover plan's own receipts depend on. A ratchet is
/// green today, cannot silently regress, and reports the remaining worklist on
/// every run.
///
/// **Lower this as F0 slices land. Never raise it.** Raising it means a
/// consumed longhand stopped resolving, which is an F0 regression, not a new
/// baseline. When it reaches 0, replace the ratchet with a plain equality
/// assertion and F0's receipt is closed.
///
/// 38 as of 2026-07-25, matching the plan's F0 table.
const MAX_REMAINING: usize = 38;

/// F0's receipt, as a ratchet. Prints the exact remaining worklist with the
/// catalog group for each name, so a failure message is the next slice.
#[test]
fn f0_receipt_consumed_longhands_are_implemented() {
    let consumed = consumed_set();
    let missing: Vec<&Consumed> = consumed
        .iter()
        .filter(|entry| !is_covered(&entry.name))
        .collect();
    let implemented = consumed.len() - missing.len();

    let mut report = format!(
        "F0 consumed-set parity: {implemented}/{} implemented, {} remaining \
         (ratchet allows {MAX_REMAINING}).\n",
        consumed.len(),
        missing.len(),
    );
    let mut uncatalogued = Vec::new();
    for entry in &missing {
        match livery::unimplemented_longhand(&entry.name) {
            Some(known) => report.push_str(&format!(
                "  {} (group {}, consumers {})\n",
                entry.name, known.group, entry.consumers,
            )),
            None => uncatalogued.push(entry.name.as_str()),
        }
    }

    // A consumed name that is in neither table is worse than an unimplemented
    // one: the census cannot count it, so it is invisible to the property
    // space and to F0's progress number. Never allowed, at any ratchet value.
    assert!(
        uncatalogued.is_empty(),
        "{report}\n  NOT IN THE CATALOG AT ALL (neither implemented nor \
         [[unimplemented]]): {}\n  Add catalog entries before anything else; \
         until then the census undercounts the work.",
        uncatalogued.join(", "),
    );

    assert!(
        missing.len() <= MAX_REMAINING,
        "{report}\nF0 REGRESSED: {} consumed longhands are unimplemented, but the \
         ratchet allows {MAX_REMAINING}. A consumed longhand stopped resolving. \
         Fix it rather than raising MAX_REMAINING.\nThis is F0 of \
         docs/2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md.",
        missing.len(),
    );

    if missing.len() < MAX_REMAINING {
        println!(
            "{report}\nF0 advanced: lower MAX_REMAINING in this file to {}.",
            missing.len(),
        );
    } else {
        println!("{report}");
    }
}

/// The multicol knockout (cutover plan D0, ruled 2026-07-25) removed
/// `column-count`, `column-width`, and `column-span` from the F4 parity bar.
/// It is only sound because none of them is consumed. If a future consumer
/// starts reading one, the knockout has to be revisited, and this is where
/// that shows up.
#[test]
fn the_multicol_knockout_does_not_touch_the_consumed_set() {
    let consumed = consumed_set();
    for property in ["column-count", "column-width", "column-span"] {
        assert!(
            !consumed.iter().any(|entry| entry.name == property),
            "{property} is in the consumed set, so the multicol knockout is no \
             longer sound; see D0 of the cutover plan",
        );
    }
}
