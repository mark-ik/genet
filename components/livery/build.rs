use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
};

use serde::Deserialize;

#[derive(Deserialize)]
struct Database {
    schema: u32,
    lane: String,
    engine_owner: String,
    consumer: String,
    status: String,
    sources: BTreeMap<String, Source>,
    shorthands: BTreeMap<String, Shorthand>,
    property: Vec<Property>,
}

#[derive(Deserialize)]
struct Source {
    url: String,
}

#[derive(Deserialize)]
struct Shorthand {
    css_name: Option<String>,
    longhands: Vec<String>,
    grammar: String,
    seed_values: Vec<String>,
    source: String,
}

#[derive(Deserialize)]
struct Property {
    name: String,
    value_type: String,
    inherited: bool,
    initial: String,
    grammar: String,
    seed_values: Vec<String>,
    animation: String,
    source: String,
}

fn rust_name(css_name: &str) -> String {
    css_name
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|ch| ch.to_ascii_uppercase())
                .into_iter()
                .chain(chars)
                .collect::<String>()
        })
        .collect()
}

fn literal(value: &str) -> String {
    format!("{value:?}")
}

fn string_slice(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| literal(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("&[{values}]")
}

fn rust_field(css_name: &str) -> String {
    css_name.replace('-', "_")
}

fn value_type_path(value_type: &str) -> &'static str {
    match value_type {
        "border-style" => "crate::values::BorderStyle",
        "border-width" => "crate::values::BorderWidth",
        "color" => "crate::values::Color",
        "display" => "crate::values::Display",
        "font-family" => "crate::values::FontFamily",
        "font-size" => "crate::values::FontSize",
        "font-style" => "crate::values::FontStyle",
        "font-weight" => "crate::values::FontWeight",
        "inset" => "crate::values::Inset",
        "line-height" => "crate::values::LineHeight",
        "list-style-type" => "crate::values::ListStyleType",
        "margin" => "crate::values::Margin",
        "overflow" => "crate::values::Overflow",
        "padding" => "crate::values::Padding",
        "position" => "crate::values::Position",
        "size" => "crate::values::Size",
        "text-decoration-line" => "crate::values::TextDecorationLine",
        "text-wrap-mode" => "crate::values::TextWrapMode",
        "white-space-collapse" => "crate::values::WhiteSpaceCollapse",
        "z-index" => "crate::values::ZIndex",
        _ => panic!("unsupported value_type {value_type}"),
    }
}

fn value_type_is_copy(value_type: &str) -> bool {
    value_type != "font-family"
}

fn initial_expression(property: &Property) -> &'static str {
    match (property.value_type.as_str(), property.initial.as_str()) {
        ("border-style", "none") => "crate::values::BorderStyle::None",
        ("border-width", "medium") => "crate::values::BorderWidth::Medium",
        ("color", "transparent") => "crate::values::Color::Transparent",
        ("color", "currentcolor") => "crate::values::Color::CurrentColor",
        ("color", "CanvasText") => "crate::values::Color::CanvasText",
        ("display", "inline") => "crate::values::Display::Inline",
        ("font-family", "depends-on-user-agent") => "crate::values::FontFamily::UserAgentDefault",
        ("font-size", "medium") => "crate::values::FontSize::Medium",
        ("font-style", "normal") => "crate::values::FontStyle::Normal",
        ("font-weight", "normal") => "crate::values::FontWeight::Normal",
        ("inset", "auto") => "crate::values::Inset::Auto",
        ("line-height", "normal") => "crate::values::LineHeight::Normal",
        ("list-style-type", "disc") => "crate::values::ListStyleType::Disc",
        ("margin", "0") => "crate::values::Margin::Value(crate::values::LengthPercentage::ZERO)",
        ("overflow", "visible") => "crate::values::Overflow::Visible",
        ("padding", "0") => "crate::values::Padding::ZERO",
        ("position", "static") => "crate::values::Position::Static",
        ("size", "auto") => "crate::values::Size::Auto",
        ("text-decoration-line", "none") => "crate::values::TextDecorationLine::NONE",
        ("text-wrap-mode", "wrap") => "crate::values::TextWrapMode::Wrap",
        ("white-space-collapse", "collapse") => "crate::values::WhiteSpaceCollapse::Collapse",
        ("z-index", "auto") => "crate::values::ZIndex::Auto",
        _ => panic!(
            "unsupported initial value {} for {} ({})",
            property.initial, property.name, property.value_type
        ),
    }
}

fn validate(db: &Database) {
    assert_eq!(db.schema, 1, "unsupported properties.toml schema");
    assert_eq!(
        db.property.len(),
        40,
        "the audited Cambium catalog must contain exactly 40 properties"
    );

    let mut names = BTreeSet::new();
    for property in &db.property {
        assert!(
            names.insert(property.name.as_str()),
            "duplicate property {}",
            property.name
        );
        assert!(
            !property.initial.is_empty(),
            "{} has no initial value",
            property.name
        );
        value_type_path(&property.value_type);
        initial_expression(property);
        assert!(
            !property.grammar.is_empty(),
            "{} has no grammar",
            property.name
        );
        assert!(
            !property.seed_values.is_empty(),
            "{} has no seed values",
            property.name
        );
        assert!(
            matches!(
                property.animation.as_str(),
                "by-computed-value" | "discrete"
            ),
            "{} has unsupported animation class {}",
            property.name,
            property.animation
        );
        assert!(
            db.sources.contains_key(&property.source),
            "{} cites missing source {}",
            property.name,
            property.source
        );
    }

    for (key, shorthand) in &db.shorthands {
        let css_name = shorthand
            .css_name
            .clone()
            .unwrap_or_else(|| key.replace('_', "-"));
        assert!(
            !shorthand.longhands.is_empty(),
            "{css_name} has no longhands"
        );
        assert!(!shorthand.grammar.is_empty(), "{css_name} has no grammar");
        assert!(
            !shorthand.seed_values.is_empty(),
            "{css_name} has no seed values"
        );
        assert!(
            db.sources.contains_key(&shorthand.source),
            "{css_name} cites missing source {}",
            shorthand.source
        );
        for longhand in &shorthand.longhands {
            assert!(
                names.contains(longhand.as_str()),
                "{css_name} expands to missing longhand {longhand}"
            );
        }
    }
}

fn generate(db: &Database) -> String {
    let mut out = String::from(
        "// @generated by components/livery/build.rs from properties.toml.\n\
         // Do not edit this output directly.\n\n",
    );

    out.push_str(&format!(
        "pub const DATABASE_SCHEMA: u32 = {};\n",
        db.schema
    ));
    out.push_str(&format!("pub const LANE: &str = {};\n", literal(&db.lane)));
    out.push_str(&format!(
        "pub const ENGINE_OWNER: &str = {};\n",
        literal(&db.engine_owner)
    ));
    out.push_str(&format!(
        "pub const CONSUMER: &str = {};\n",
        literal(&db.consumer)
    ));
    out.push_str(&format!(
        "pub const CATALOG_STATUS: &str = {};\n\n",
        literal(&db.status)
    ));

    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]\n\
         pub enum AnimationClass {\n    ByComputedValue,\n    Discrete,\n}\n\n",
    );
    let value_types = db
        .property
        .iter()
        .map(|property| property.value_type.as_str())
        .collect::<BTreeSet<_>>();
    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]\n\
         pub enum ValueType {\n",
    );
    for value_type in &value_types {
        out.push_str(&format!("    {},\n", rust_name(value_type)));
    }
    out.push_str("}\n\n");
    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
         pub struct PropertyMetadata {\n\
         \x20   pub id: PropertyId,\n\
         \x20   pub name: &'static str,\n\
         \x20   pub value_type: ValueType,\n\
         \x20   pub inherited: bool,\n\
         \x20   pub initial: &'static str,\n\
         \x20   pub grammar: &'static str,\n\
         \x20   pub seed_values: &'static [&'static str],\n\
         \x20   pub animation: AnimationClass,\n\
         \x20   pub source_url: &'static str,\n\
         }\n\n",
    );
    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]\n\
         #[repr(u8)]\n\
         pub enum PropertyId {\n",
    );
    for property in &db.property {
        out.push_str(&format!("    {},\n", rust_name(&property.name)));
    }
    out.push_str("}\n\nimpl PropertyId {\n    pub const ALL: &'static [Self] = &[\n");
    for property in &db.property {
        out.push_str(&format!("        Self::{},\n", rust_name(&property.name)));
    }
    out.push_str(
        "    ];\n\n    pub fn from_css_name(name: &str) -> Option<Self> {\n        match name {\n",
    );
    for property in &db.property {
        out.push_str(&format!(
            "            {} => Some(Self::{}),\n",
            literal(&property.name),
            rust_name(&property.name)
        ));
    }
    out.push_str("            _ => None,\n        }\n    }\n\n    pub const fn metadata(self) -> PropertyMetadata {\n        match self {\n");
    for property in &db.property {
        let animation = match property.animation.as_str() {
            "by-computed-value" => "AnimationClass::ByComputedValue",
            "discrete" => "AnimationClass::Discrete",
            _ => unreachable!("validated animation class"),
        };
        let source = &db.sources[&property.source].url;
        out.push_str(&format!(
            "            Self::{variant} => PropertyMetadata {{ id: Self::{variant}, name: {name}, value_type: ValueType::{value_type}, inherited: {inherited}, initial: {initial}, grammar: {grammar}, seed_values: {seed_values}, animation: {animation}, source_url: {source} }},\n",
            variant = rust_name(&property.name),
            name = literal(&property.name),
            value_type = rust_name(&property.value_type),
            inherited = property.inherited,
            initial = literal(&property.initial),
            grammar = literal(&property.grammar),
            seed_values = string_slice(&property.seed_values),
            source = literal(source),
        ));
    }
    out.push_str("        }\n    }\n}\n\n");

    out.push_str("#[derive(Clone, Debug, PartialEq)]\npub enum PropertyValue {\n");
    for value_type in &value_types {
        out.push_str(&format!(
            "    {}({}),\n",
            rust_name(value_type),
            value_type_path(value_type)
        ));
    }
    out.push_str("}\n\nimpl PropertyValue {\n    pub fn parse(property: PropertyId, input: &str) -> Result<Self, crate::values::ParseError> {\n        match property {\n");
    for property in &db.property {
        out.push_str(&format!(
            "            PropertyId::{property} => input.parse::<{value_path}>().map(Self::{value_variant}),\n",
            property = rust_name(&property.name),
            value_path = value_type_path(&property.value_type),
            value_variant = rust_name(&property.value_type),
        ));
    }
    out.push_str("        }\n    }\n\n    pub const fn value_type(&self) -> ValueType {\n        match self {\n");
    for value_type in &value_types {
        let variant = rust_name(value_type);
        out.push_str(&format!(
            "            Self::{variant}(..) => ValueType::{variant},\n"
        ));
    }
    out.push_str(
        "        }\n    }\n\n    pub fn to_css_string(&self) -> String {\n        match self {\n",
    );
    for value_type in &value_types {
        out.push_str(&format!(
            "            Self::{}(value) => value.to_string(),\n",
            rust_name(value_type)
        ));
    }
    out.push_str("        }\n    }\n}\n\n");

    out.push_str("#[derive(Clone, Debug, PartialEq)]\npub struct ComputedValues {\n");
    for property in &db.property {
        out.push_str(&format!(
            "    pub {}: {},\n",
            rust_field(&property.name),
            value_type_path(&property.value_type)
        ));
    }
    out.push_str(
        "}\n\nimpl Default for ComputedValues {\n    fn default() -> Self {\n        Self {\n",
    );
    for property in &db.property {
        out.push_str(&format!(
            "            {}: {},\n",
            rust_field(&property.name),
            initial_expression(property)
        ));
    }
    out.push_str("        }\n    }\n}\n\nimpl ComputedValues {\n    /// Construct a child style with inherited properties copied from `parent`\n    /// and non-inherited properties reset to their CSS initial values.\n    pub fn for_child(parent: &Self) -> Self {\n        let initial = Self::default();\n        Self {\n");
    for property in &db.property {
        let field = rust_field(&property.name);
        if property.inherited {
            if value_type_is_copy(&property.value_type) {
                out.push_str(&format!("            {field}: parent.{field},\n"));
            } else {
                out.push_str(&format!("            {field}: parent.{field}.clone(),\n"));
            }
        } else {
            out.push_str(&format!("            {field}: initial.{field},\n"));
        }
    }
    out.push_str("        }\n    }\n\n    /// Assign one generated property, returning the value unchanged on a type mismatch.\n    pub fn set(&mut self, property: PropertyId, value: PropertyValue) -> Result<(), PropertyValue> {\n        match (property, value) {\n");
    for property in &db.property {
        let property_variant = rust_name(&property.name);
        let value_variant = rust_name(&property.value_type);
        let field = rust_field(&property.name);
        out.push_str(&format!(
            "            (PropertyId::{property_variant}, PropertyValue::{value_variant}(value)) => {{ self.{field} = value; Ok(()) }},\n"
        ));
    }
    out.push_str("            (_, value) => Err(value),\n        }\n    }\n\n    /// Copy one property from another computed style.\n    pub fn copy_property_from(&mut self, property: PropertyId, source: &Self) {\n        match property {\n");
    for property in &db.property {
        let property_variant = rust_name(&property.name);
        let field = rust_field(&property.name);
        if value_type_is_copy(&property.value_type) {
            out.push_str(&format!(
                "            PropertyId::{property_variant} => self.{field} = source.{field},\n"
            ));
        } else {
            out.push_str(&format!(
                "            PropertyId::{property_variant} => self.{field} = source.{field}.clone(),\n"
            ));
        }
    }
    out.push_str("        }\n    }\n}\n\n");

    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
         pub struct ShorthandMetadata {\n\
         \x20   pub id: ShorthandId,\n\
         \x20   pub name: &'static str,\n\
         \x20   pub longhands: &'static [PropertyId],\n\
         \x20   pub grammar: &'static str,\n\
         \x20   pub seed_values: &'static [&'static str],\n\
         \x20   pub source_url: &'static str,\n\
         }\n\n",
    );
    out.push_str(
        "#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]\n\
         pub enum ShorthandId {\n",
    );
    for (key, shorthand) in &db.shorthands {
        let name = shorthand
            .css_name
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| key.replace('_', "-"));
        out.push_str(&format!("    {},\n", rust_name(&name)));
    }
    out.push_str("}\n\nimpl ShorthandId {\n    pub const ALL: &'static [Self] = &[\n");
    for (key, shorthand) in &db.shorthands {
        let name = shorthand
            .css_name
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| key.replace('_', "-"));
        out.push_str(&format!("        Self::{},\n", rust_name(&name)));
    }
    out.push_str(
        "    ];\n\n    pub fn from_css_name(name: &str) -> Option<Self> {\n        match name {\n",
    );
    for (key, shorthand) in &db.shorthands {
        let name = shorthand
            .css_name
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| key.replace('_', "-"));
        out.push_str(&format!(
            "            {} => Some(Self::{}),\n",
            literal(&name),
            rust_name(&name)
        ));
    }
    out.push_str("            _ => None,\n        }\n    }\n\n    pub const fn metadata(self) -> ShorthandMetadata {\n        match self {\n");
    for (key, shorthand) in &db.shorthands {
        let name = shorthand
            .css_name
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| key.replace('_', "-"));
        let longhands = shorthand
            .longhands
            .iter()
            .map(|name| format!("PropertyId::{}", rust_name(name)))
            .collect::<Vec<_>>()
            .join(", ");
        let source = &db.sources[&shorthand.source].url;
        out.push_str(&format!(
            "            Self::{variant} => ShorthandMetadata {{ id: Self::{variant}, name: {name}, longhands: &[{longhands}], grammar: {grammar}, seed_values: {seed_values}, source_url: {source} }},\n",
            variant = rust_name(&name),
            name = literal(&name),
            grammar = literal(&shorthand.grammar),
            seed_values = string_slice(&shorthand.seed_values),
            source = literal(source),
        ));
    }
    out.push_str("        }\n    }\n}\n");
    out
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let database_path = manifest_dir.join("properties.toml");
    println!("cargo:rerun-if-changed={}", database_path.display());

    let source = fs::read_to_string(&database_path).expect("read properties.toml");
    let database: Database = toml::from_str(&source).expect("parse properties.toml");
    validate(&database);

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out_dir.join("properties.rs"), generate(&database))
        .expect("write generated properties.rs");
}
