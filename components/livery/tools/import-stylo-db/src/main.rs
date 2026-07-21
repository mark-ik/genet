//! Harvest H0: populate livery's property space from the stylo fork's
//! property database, as data.
//!
//! Reads `longhands.toml` + `shorthands.toml` from the fork checkout,
//! keeps the servo-lane entries (drops `engine = "gecko"`), and rewrites
//! the marked generated section at the end of livery's `properties.toml`
//! with `[[unimplemented]]` / `[[unimplemented_shorthand]]` entries for
//! every servo-lane name livery does not implement yet. Also writes the
//! `PROPERTY_SPACE.md` census. Inheritance derives from the fork's style
//! struct table (`data.py`: font, inherited_*, list are the inherited
//! structs); the struct name is preserved as `group`, the future
//! ComputedValues grouping seam.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::exit;

const BEGIN_MARKER: &str =
    "# ==== BEGIN GENERATED property-space import (stylo harvest H0) ====";
const END_MARKER: &str = "# ==== END GENERATED property-space import ====";

const INHERITED_GROUPS: &[&str] = &[
    "font",
    "inherited_box",
    "inherited_svg",
    "inherited_table",
    "inherited_text",
    "inherited_ui",
    "list",
];

struct Longhand {
    name: String,
    group: String,
    inherited: bool,
    animation: &'static str,
    logical: bool,
    aliases: Vec<String>,
    spec: String,
}

struct Shorthand {
    name: String,
    sub_properties: Vec<String>,
    aliases: Vec<String>,
    spec: String,
}

fn is_servo_lane(entry: &toml::Table) -> bool {
    match entry.get("engine").and_then(|value| value.as_str()) {
        None | Some("servo") => true,
        Some(_) => false,
    }
}

fn string_list(entry: &toml::Table, key: &str) -> Vec<String> {
    entry
        .get(key)
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn toml_string(value: &str) -> String {
    format!("{:?}", value)
}

fn main() {
    let mut stylo_properties: Option<PathBuf> = None;
    let mut source_rev = String::from("unrecorded");
    let mut livery_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stylo-properties" => stylo_properties = args.next().map(PathBuf::from),
            "--source-rev" => source_rev = args.next().unwrap_or_default(),
            "--livery-dir" => {
                if let Some(dir) = args.next() {
                    livery_dir = PathBuf::from(dir);
                }
            },
            other => {
                eprintln!("unknown argument {other}");
                exit(2);
            },
        }
    }
    let Some(stylo_properties) = stylo_properties else {
        eprintln!(
            "usage: import-stylo-db --stylo-properties <stylo>/style/properties \
             [--source-rev <sha>] [--livery-dir <path>]"
        );
        exit(2);
    };

    let read_table = |name: &str| -> toml::Table {
        let path = stylo_properties.join(name);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        source
            .parse::<toml::Table>()
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
    };

    let stylo_longhands = read_table("longhands.toml");
    let stylo_shorthands = read_table("shorthands.toml");

    let mut gecko_only_longhands = 0usize;
    let mut longhands = Vec::new();
    for (name, entry) in &stylo_longhands {
        let entry = entry.as_table().expect("longhand entry is a table");
        if !is_servo_lane(entry) {
            gecko_only_longhands += 1;
            continue;
        }
        let group = entry
            .get("struct")
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| panic!("{name} has no struct"))
            .to_owned();
        let animation = match entry
            .get("animation_type")
            .and_then(|value| value.as_str())
            .unwrap_or("normal")
        {
            "normal" => "by-computed-value",
            "discrete" => "discrete",
            "none" => "none",
            other => panic!("{name} has unknown animation_type {other}"),
        };
        longhands.push(Longhand {
            name: name.clone(),
            inherited: INHERITED_GROUPS.contains(&group.as_str()),
            group,
            animation,
            logical: entry
                .get("logical")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            aliases: string_list(entry, "aliases"),
            spec: entry
                .get("spec")
                .and_then(|value| value.as_str())
                .unwrap_or_else(|| panic!("{name} has no spec"))
                .to_owned(),
        });
    }

    let mut gecko_only_shorthands = 0usize;
    let mut shorthands = Vec::new();
    for (name, entry) in &stylo_shorthands {
        let entry = entry.as_table().expect("shorthand entry is a table");
        if !is_servo_lane(entry) {
            gecko_only_shorthands += 1;
            continue;
        }
        shorthands.push(Shorthand {
            name: name.clone(),
            sub_properties: string_list(entry, "sub_properties"),
            aliases: string_list(entry, "aliases"),
            spec: entry
                .get("spec")
                .and_then(|value| value.as_str())
                .unwrap_or_else(|| panic!("{name} has no spec"))
                .to_owned(),
        });
    }

    let database_path = livery_dir.join("properties.toml");
    let database_source = std::fs::read_to_string(&database_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", database_path.display()));
    let database: toml::Table = database_source
        .parse()
        .unwrap_or_else(|error| panic!("parse {}: {error}", database_path.display()));

    let implemented_longhands: BTreeSet<String> = database
        .get("property")
        .and_then(|value| value.as_array())
        .expect("properties.toml has [[property]] entries")
        .iter()
        .filter_map(|entry| entry.get("name").and_then(|name| name.as_str()))
        .map(str::to_owned)
        .collect();
    let implemented_shorthands: BTreeSet<String> = database
        .get("shorthands")
        .and_then(|value| value.as_table())
        .expect("properties.toml has [shorthands.*] entries")
        .iter()
        .map(|(key, entry)| {
            entry
                .get("css_name")
                .and_then(|name| name.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| key.replace('_', "-"))
        })
        .collect();

    let servo_longhand_names: BTreeSet<&str> =
        longhands.iter().map(|longhand| longhand.name.as_str()).collect();
    let servo_shorthand_names: BTreeSet<&str> =
        shorthands.iter().map(|shorthand| shorthand.name.as_str()).collect();

    // A name livery implements as the other kind (its bounded
    // `background-position` longhand vs upstream's shorthand) is covered,
    // not unimplemented: the parser resolves the name, so the
    // known-unimplemented diagnostic must never claim it.
    let cross_kind_longhands: Vec<&Longhand> = longhands
        .iter()
        .filter(|longhand| implemented_shorthands.contains(&longhand.name))
        .collect();
    let cross_kind_shorthands: Vec<&Shorthand> = shorthands
        .iter()
        .filter(|shorthand| implemented_longhands.contains(&shorthand.name))
        .collect();
    let unimplemented_longhands: Vec<&Longhand> = longhands
        .iter()
        .filter(|longhand| {
            !implemented_longhands.contains(&longhand.name)
                && !implemented_shorthands.contains(&longhand.name)
        })
        .collect();
    let unimplemented_shorthands: Vec<&Shorthand> = shorthands
        .iter()
        .filter(|shorthand| {
            !implemented_shorthands.contains(&shorthand.name)
                && !implemented_longhands.contains(&shorthand.name)
        })
        .collect();
    let livery_local_longhands: Vec<&String> = implemented_longhands
        .iter()
        .filter(|name| !servo_longhand_names.contains(name.as_str()))
        .collect();
    let livery_local_shorthands: Vec<&String> = implemented_shorthands
        .iter()
        .filter(|name| !servo_shorthand_names.contains(name.as_str()))
        .collect();

    // Rewrite the generated section of properties.toml.
    let base = match database_source.find(BEGIN_MARKER) {
        Some(index) => database_source[..index].trim_end().to_owned(),
        None => database_source.trim_end().to_owned(),
    };
    let mut section = String::new();
    section.push_str(BEGIN_MARKER);
    section.push_str(&format!(
        "\n# @generated by tools/import-stylo-db from the stylo fork's servo-lane\n\
         # longhands.toml + shorthands.toml (mark-ik/stylo, branch genet-rename,\n\
         # rev {source_rev}). Known to the catalog, rejected by the parser with a\n\
         # known-unimplemented diagnostic, and generated into the unimplemented\n\
         # metadata tables. `group` is the fork's style struct: the future\n\
         # ComputedValues grouping seam. Re-run the tool after fork realignments;\n\
         # entries leave this section by being implemented as [[property]] rows.\n\n"
    ));

    let mut by_group: BTreeMap<&str, Vec<&Longhand>> = BTreeMap::new();
    for longhand in &unimplemented_longhands {
        by_group.entry(longhand.group.as_str()).or_default().push(longhand);
    }
    for (group, group_longhands) in &by_group {
        section.push_str(&format!("# group: {group}\n"));
        for longhand in group_longhands {
            section.push_str("[[unimplemented]]\n");
            section.push_str(&format!("name = {}\n", toml_string(&longhand.name)));
            section.push_str(&format!("group = {}\n", toml_string(&longhand.group)));
            section.push_str(&format!("inherited = {}\n", longhand.inherited));
            section.push_str(&format!("animation = {}\n", toml_string(longhand.animation)));
            if longhand.logical {
                section.push_str("logical = true\n");
            }
            if !longhand.aliases.is_empty() {
                let aliases = longhand
                    .aliases
                    .iter()
                    .map(|alias| toml_string(alias))
                    .collect::<Vec<_>>()
                    .join(", ");
                section.push_str(&format!("aliases = [{aliases}]\n"));
            }
            section.push_str(&format!("spec = {}\n\n", toml_string(&longhand.spec)));
        }
    }
    for shorthand in &unimplemented_shorthands {
        section.push_str("[[unimplemented_shorthand]]\n");
        section.push_str(&format!("name = {}\n", toml_string(&shorthand.name)));
        let sub_properties = shorthand
            .sub_properties
            .iter()
            .map(|sub| toml_string(sub))
            .collect::<Vec<_>>()
            .join(", ");
        section.push_str(&format!("sub_properties = [{sub_properties}]\n"));
        if !shorthand.aliases.is_empty() {
            let aliases = shorthand
                .aliases
                .iter()
                .map(|alias| toml_string(alias))
                .collect::<Vec<_>>()
                .join(", ");
            section.push_str(&format!("aliases = [{aliases}]\n"));
        }
        section.push_str(&format!("spec = {}\n\n", toml_string(&shorthand.spec)));
    }
    section.push_str(END_MARKER);
    section.push('\n');

    std::fs::write(&database_path, format!("{base}\n\n{section}"))
        .unwrap_or_else(|error| panic!("write {}: {error}", database_path.display()));

    // Census.
    let mut census = String::new();
    census.push_str(&format!(
        "# Livery property space census\n\n\
         @generated by `tools/import-stylo-db` from the stylo fork property\n\
         database (mark-ik/stylo, branch genet-rename, rev {source_rev}).\n\
         Regenerate after fork realignments or livery property additions.\n\n\
         | space | longhands | shorthands |\n\
         | --- | --- | --- |\n\
         | stylo full database | {} | {} |\n\
         | excluded (gecko-only engine) | {gecko_only_longhands} | {gecko_only_shorthands} |\n\
         | servo-lane destination | {} | {} |\n\
         | implemented in livery | {} | {} |\n\
         | of which livery-local (outside the servo lane) | {} | {} |\n\
         | remaining (known, unimplemented) | {} | {} |\n\n",
        stylo_longhands.len(),
        stylo_shorthands.len(),
        longhands.len(),
        shorthands.len(),
        implemented_longhands.len(),
        implemented_shorthands.len(),
        livery_local_longhands.len(),
        livery_local_shorthands.len(),
        unimplemented_longhands.len(),
        unimplemented_shorthands.len(),
    ));
    census.push_str(
        "Livery-local names are livery catalog entries with no same-name servo-lane\n\
         longhand or shorthand (bounded simplifications such as a single `overflow`\n\
         longhand). They are not wrong; they are seams to reconcile as their\n\
         families grow.\n\n## Livery-local entries\n\n",
    );
    for name in &livery_local_longhands {
        census.push_str(&format!("- `{name}` (longhand)\n"));
    }
    for name in &livery_local_shorthands {
        census.push_str(&format!("- `{name}` (shorthand)\n"));
    }
    census.push_str(
        "\n## Covered cross-kind\n\nNames livery models as the other kind; the parser \
         resolves them, so they\nare covered, with the upstream decomposition still \
         ahead of them.\n\n",
    );
    for longhand in &cross_kind_longhands {
        census.push_str(&format!(
            "- `{}` (upstream longhand; livery shorthand)\n",
            longhand.name
        ));
    }
    for shorthand in &cross_kind_shorthands {
        census.push_str(&format!(
            "- `{}` (upstream shorthand -> {}; livery longhand)\n",
            shorthand.name,
            shorthand
                .sub_properties
                .iter()
                .map(|sub| format!("`{sub}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    census.push_str("\n## Remaining longhands by group\n");
    for (group, group_longhands) in &by_group {
        census.push_str(&format!("\n### {group} ({})\n\n", group_longhands.len()));
        for longhand in group_longhands {
            let mut notes = Vec::new();
            if longhand.inherited {
                notes.push("inherited");
            }
            if longhand.logical {
                notes.push("logical");
            }
            if longhand.animation == "discrete" {
                notes.push("discrete");
            } else if longhand.animation == "none" {
                notes.push("not animatable");
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join(", "))
            };
            census.push_str(&format!("- `{}`{notes}\n", longhand.name));
        }
    }
    census.push_str(&format!(
        "\n## Remaining shorthands ({})\n\n",
        unimplemented_shorthands.len()
    ));
    for shorthand in &unimplemented_shorthands {
        census.push_str(&format!(
            "- `{}` -> {}\n",
            shorthand.name,
            shorthand
                .sub_properties
                .iter()
                .map(|sub| format!("`{sub}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    census.push_str(
        "\n## Out of scope for the property space\n\n\
         The fork's descriptor databases (`font_face_descriptors.toml`,\n\
         `counter_style_descriptors.toml`, `property_descriptors.toml`,\n\
         `view_transition_descriptors.toml`) describe at-rule descriptors, not\n\
         properties; they enter with their subsystems (H1 `@property`, H5 fonts).\n",
    );

    let census_path = livery_dir.join("PROPERTY_SPACE.md");
    std::fs::write(&census_path, census)
        .unwrap_or_else(|error| panic!("write {}: {error}", census_path.display()));

    println!(
        "servo-lane {}+{} | implemented {}+{} | unimplemented {}+{} | livery-local {}+{}",
        longhands.len(),
        shorthands.len(),
        implemented_longhands.len(),
        implemented_shorthands.len(),
        unimplemented_longhands.len(),
        unimplemented_shorthands.len(),
        livery_local_longhands.len(),
        livery_local_shorthands.len(),
    );
}
