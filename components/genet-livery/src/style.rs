use std::{collections::HashMap, hash::Hash};

use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
use livery::{
    ComputedValues,
    cascade::{
        CascadeLayer, DeclarationError, MatchedCustomDeclaration, MatchedDeclaration, Origin,
        Specificity, cascade_with_custom, parse_declaration_block,
    },
    custom::CustomProperties,
    media::Device,
    stylesheet::{Keyframes, StyleRule, Stylesheet, StylesheetDiagnostic},
    values::{FontSize, Length, LengthPercentage, LengthUnit, LineHeight},
};

use crate::{CAMBIUM_UA_DEFAULTS, InteractionStates, SelectorTree};

/// Parsed UA and author rules for one document class.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleSet {
    rules: Vec<StyleRule>,
    keyframes: Vec<Keyframes>,
    diagnostics: Vec<StylesheetDiagnostic>,
}

impl StyleSet {
    pub fn cambium(author_sheets: &[&str]) -> Self {
        Self::parse(CAMBIUM_UA_DEFAULTS, author_sheets)
    }

    pub fn parse(ua_sheet: &str, author_sheets: &[&str]) -> Self {
        let mut result = Self::default();
        let ua = Stylesheet::parse(ua_sheet, Origin::UserAgent);
        result.diagnostics.extend_from_slice(ua.diagnostics());
        result.rules.extend(ua.rules().iter().cloned());
        result.keyframes.extend(ua.keyframes().iter().cloned());

        let mut source_order = 0_u64;
        for source in author_sheets {
            let author = Stylesheet::parse_with_offset(source, Origin::Author, source_order);
            source_order = source_order.saturating_add(author.rules().len() as u64);
            result.diagnostics.extend_from_slice(author.diagnostics());
            result.rules.extend(author.rules().iter().cloned());
            result.keyframes.extend(author.keyframes().iter().cloned());
        }
        result
    }

    pub fn rules(&self) -> &[StyleRule] {
        &self.rules
    }

    pub fn diagnostics(&self) -> &[StylesheetDiagnostic] {
        &self.diagnostics
    }

    pub(crate) fn keyframes(&self, name: &str) -> Option<&Keyframes> {
        self.keyframes
            .iter()
            .rev()
            .find(|keyframes| keyframes.name().eq_ignore_ascii_case(name))
    }
}

/// Concrete Livery computed styles keyed by the source DOM node.
#[derive(Clone, Debug)]
pub struct StylePlane<Id> {
    values: HashMap<Id, ComputedValues>,
    custom: HashMap<Id, CustomProperties>,
    inline_diagnostics: HashMap<Id, Vec<DeclarationError>>,
}

impl<Id> Default for StylePlane<Id> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            custom: HashMap::new(),
            inline_diagnostics: HashMap::new(),
        }
    }
}

impl<Id> StylePlane<Id>
where
    Id: Eq + Hash,
{
    pub fn get(&self, id: Id) -> Option<&ComputedValues> {
        self.values.get(&id)
    }

    /// The element's computed custom-property map (harvest H1).
    pub fn custom_properties(&self, id: Id) -> Option<&CustomProperties> {
        self.custom.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: Id) -> Option<&mut ComputedValues> {
        self.values.get_mut(&id)
    }

    pub fn inline_diagnostics(&self, id: Id) -> &[DeclarationError] {
        self.inline_diagnostics.get(&id).map_or(&[], Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Resolve every element in a neutral Genet DOM through Livery.
pub fn resolve_styles<D>(
    dom: &D,
    style_set: &StyleSet,
    device: &Device,
    states: &InteractionStates<D::NodeId>,
) -> StylePlane<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let selector_tree = SelectorTree::new(dom, states);
    let mut plane = StylePlane::default();
    resolve_subtree(
        &selector_tree,
        style_set,
        device,
        dom.document(),
        None,
        None,
        &mut plane,
    );
    plane
}

fn resolve_subtree<D>(
    selector_tree: &SelectorTree<'_, D>,
    style_set: &StyleSet,
    device: &Device,
    id: D::NodeId,
    parent: Option<&ComputedValues>,
    parent_custom: Option<&CustomProperties>,
    plane: &mut StylePlane<D::NodeId>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if selector_tree.dom().kind(id) == NodeKind::Element {
        let element = selector_tree.element(id).expect("element kind has adapter");
        let mut matched = Vec::new();
        let mut matched_custom = Vec::new();
        for rule in &style_set.rules {
            matched.extend(rule.matched_declarations(&element, device));
            matched_custom.extend(rule.matched_custom_declarations(&element, device));
        }

        if let Some(inline) =
            selector_tree
                .dom()
                .attribute(id, &Namespace::from(""), &LocalName::from("style"))
        {
            let block = parse_declaration_block(inline);
            if !block.errors.is_empty() {
                plane.inline_diagnostics.insert(id, block.errors);
            }
            let inline_order = u64::MAX.saturating_sub(65_535);
            matched.extend(block.declarations.into_iter().enumerate().map(
                |(index, declaration)| MatchedDeclaration {
                    declaration,
                    origin: Origin::Author,
                    layer: CascadeLayer::Unlayered,
                    specificity: Specificity::INLINE,
                    source_order: inline_order.saturating_add(index as u64),
                },
            ));
            matched_custom.extend(block.custom.into_iter().enumerate().map(
                |(index, declaration)| MatchedCustomDeclaration {
                    declaration,
                    origin: Origin::Author,
                    layer: CascadeLayer::Unlayered,
                    specificity: Specificity::INLINE,
                    source_order: inline_order.saturating_add(index as u64),
                },
            ));
        }

        let (mut computed, custom) =
            cascade_with_custom(parent, parent_custom, matched, matched_custom);
        resolve_font_metrics(&mut computed, parent);
        for child in selector_tree.dom().dom_children(id) {
            resolve_subtree(
                selector_tree,
                style_set,
                device,
                child,
                Some(&computed),
                Some(&custom),
                plane,
            );
        }
        plane.values.insert(id, computed);
        plane.custom.insert(id, custom);
    } else {
        for child in selector_tree.dom().dom_children(id) {
            resolve_subtree(
                selector_tree,
                style_set,
                device,
                child,
                parent,
                parent_custom,
                plane,
            );
        }
    }
}

fn resolve_font_metrics(computed: &mut ComputedValues, parent: Option<&ComputedValues>) {
    let parent_size = parent.map_or(16.0, |style| match style.font_size {
        FontSize::Value(LengthPercentage::Length(Length {
            value,
            unit: LengthUnit::Px,
        })) => value,
        _ => 16.0,
    });
    let font_size = match computed.font_size {
        FontSize::Medium => 16.0,
        FontSize::Value(value) => resolve_length_percentage(value, parent_size, parent_size),
    }
    .max(0.0);
    computed.font_size = FontSize::Value(LengthPercentage::Length(Length::px(font_size)));

    if let LineHeight::Value(value) = computed.line_height {
        computed.line_height = LineHeight::Value(LengthPercentage::Length(Length::px(
            resolve_length_percentage(value, font_size, font_size).max(0.0),
        )));
    }
}

fn resolve_length_percentage(value: LengthPercentage, em: f32, percentage_basis: f32) -> f32 {
    match value {
        LengthPercentage::Zero => 0.0,
        LengthPercentage::Length(length) => length.unit.to_px(length.value, em, 16.0),
        LengthPercentage::Percentage(value) => percentage_basis * value,
        LengthPercentage::Calc(calc) => {
            percentage_basis * calc.percentage + calc.px + calc.em * em + calc.rem * 16.0
        },
    }
}
