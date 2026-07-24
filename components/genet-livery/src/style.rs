use std::{collections::HashMap, hash::Hash};

use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
use livery::{
    ComputedValues, PropertyId,
    cascade::{
        CascadeLayer, DeclarationError, MatchedCustomDeclaration, MatchedDeclaration, Origin,
        Specificity, cascade_with_custom, parse_declaration_block,
    },
    custom::CustomProperties,
    media::Device,
    stylesheet::{
        ContainerSnapshot, Keyframes, RuleMutationError, StyleRule, Stylesheet,
        StylesheetDiagnostic,
    },
    values::{
        BorderStyle, FontSize, Length, LengthPercentage, LengthUnit, LineHeight, Margin, Padding,
        Size,
    },
};

use crate::{CAMBIUM_UA_DEFAULTS, InteractionStates, SelectorTree};

/// Layout facts needed to serialize properties whose CSSOM result is a used
/// value rather than only a computed value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsedValueContext {
    pub border_box: (f32, f32),
    pub containing_inline_size: Option<f32>,
}

/// Parsed UA and author rules for one document class. The sheets are
/// retained as CSSOM-shaped objects (harvest H3); the flattened rule and
/// keyframes views are rebuilt after every mutation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleSet {
    ua: Stylesheet,
    authors: Vec<Stylesheet>,
    rules: Vec<StyleRule>,
    keyframes: Vec<Keyframes>,
    diagnostics: Vec<StylesheetDiagnostic>,
}

impl StyleSet {
    pub fn cambium(author_sheets: &[&str]) -> Self {
        Self::parse(CAMBIUM_UA_DEFAULTS, author_sheets)
    }

    pub fn parse(ua_sheet: &str, author_sheets: &[&str]) -> Self {
        let mut result = Self {
            ua: Stylesheet::parse(ua_sheet, Origin::UserAgent),
            ..Self::default()
        };
        for source in author_sheets {
            result
                .authors
                .push(Stylesheet::parse(source, Origin::Author));
        }
        result.rebuild();
        result
    }

    /// Rebuild the flattened cascade views from the retained sheets, with
    /// author source order running across sheets in document order.
    fn rebuild(&mut self) {
        self.rules.clear();
        self.keyframes.clear();
        self.diagnostics.clear();
        self.diagnostics.extend_from_slice(self.ua.diagnostics());
        self.rules.extend(self.ua.reindexed_rules(0));
        self.keyframes.extend(self.ua.keyframes().iter().cloned());
        let mut source_order = 0_u64;
        for author in &self.authors {
            self.diagnostics.extend_from_slice(author.diagnostics());
            self.rules.extend(author.reindexed_rules(source_order));
            source_order = source_order.saturating_add(author.rules().len() as u64);
            self.keyframes.extend(author.keyframes().iter().cloned());
        }
    }

    /// The retained author sheets, in document order.
    pub fn author_sheets(&self) -> &[Stylesheet] {
        &self.authors
    }

    /// Aggregate monotonic sheet stamp for consumers retaining a style plane.
    pub fn generation(&self) -> u64 {
        self.authors
            .iter()
            .fold(self.ua.generation(), |generation, sheet| {
                generation.saturating_add(sheet.generation())
            })
    }

    pub(crate) fn has_sibling_dependencies(&self) -> bool {
        self.rules.iter().any(StyleRule::has_sibling_dependency)
    }

    pub(crate) fn has_structural_dependencies(&self) -> bool {
        self.rules.iter().any(StyleRule::has_structural_dependency)
    }

    pub(crate) fn has_container_queries(&self) -> bool {
        self.rules.iter().any(StyleRule::has_container_query)
    }

    /// CSSOM `insertRule` on one author sheet; the cascade views rebuild.
    pub fn insert_author_rule(
        &mut self,
        sheet: usize,
        rule: &str,
        index: usize,
    ) -> Result<usize, RuleMutationError> {
        let target = self
            .authors
            .get_mut(sheet)
            .ok_or(RuleMutationError::IndexSize)?;
        let inserted = target.insert_rule(rule, index)?;
        self.rebuild();
        Ok(inserted)
    }

    /// CSSOM `deleteRule` on one author sheet; the cascade views rebuild.
    pub fn delete_author_rule(
        &mut self,
        sheet: usize,
        index: usize,
    ) -> Result<(), RuleMutationError> {
        let target = self
            .authors
            .get_mut(sheet)
            .ok_or(RuleMutationError::IndexSize)?;
        target.delete_rule(index)?;
        self.rebuild();
        Ok(())
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

impl<Id> PartialEq for StylePlane<Id>
where
    Id: Eq + Hash,
{
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
            && self.custom == other.custom
            && self.inline_diagnostics == other.inline_diagnostics
    }
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

    /// Serialize one computed longhand or custom property from this plane.
    /// This is shared by retained documents and scripted on-demand reads, so
    /// both JS and native CSSOM surfaces use the generated H2 value dispatch.
    pub fn computed_style(&self, id: Id, property: &str) -> Option<String> {
        self.computed_style_with_used_size(id, property, None)
    }

    /// Serialize a computed value with an optional retained layout size.
    ///
    /// CSSOM exposes used pixel values for width and height. A caller that has
    /// laid out the current style plane can supply that border-box size; the
    /// bounded lane uses it only when no padding or border changes the
    /// relationship between the fragment and the property value.
    pub fn computed_style_with_used_size(
        &self,
        id: Id,
        property: &str,
        used_size: Option<(f32, f32)>,
    ) -> Option<String> {
        self.computed_style_with_used_values(
            id,
            property,
            used_size.map(|border_box| UsedValueContext {
                border_box,
                containing_inline_size: None,
            }),
        )
    }

    /// Serialize a computed value with the layout bases needed by CSSOM used
    /// values. The current bounded surface covers box size and physical
    /// margins; other adorned-box properties remain explicit follow-ons.
    pub fn computed_style_with_used_values(
        &self,
        id: Id,
        property: &str,
        used: Option<UsedValueContext>,
    ) -> Option<String> {
        if property.starts_with("--") {
            return self.custom_properties(id)?.get(property).cloned();
        }
        let property = PropertyId::from_css_name(&property.to_ascii_lowercase())?;
        let values = self.get(id)?;
        if let Some(used) = used
            && box_is_unadorned(values)
        {
            let value = match property {
                PropertyId::Width => Some(used.border_box.0),
                PropertyId::Height => Some(used.border_box.1),
                PropertyId::MarginTop => used_margin(values.margin_top, values, used),
                PropertyId::MarginRight => used_margin(values.margin_right, values, used),
                PropertyId::MarginBottom => used_margin(values.margin_bottom, values, used),
                PropertyId::MarginLeft => used_margin(values.margin_left, values, used),
                _ => None,
            };
            if let Some(value) = value {
                return Some(used_px(value));
            }
        }
        if property == PropertyId::Transform {
            let em = match values.font_size {
                FontSize::Value(LengthPercentage::Length(Length {
                    value,
                    unit: LengthUnit::Px,
                })) => value,
                _ => 16.0,
            };
            let reference_box = definite_transform_reference_box(values, em);
            return Some(values.transform.to_computed_css(em, reference_box));
        }
        Some(values.get(property).to_css_string())
    }

    pub(crate) fn get_mut(&mut self, id: Id) -> Option<&mut ComputedValues> {
        self.values.get_mut(&id)
    }

    pub(crate) fn resolve_relative_lengths(
        &mut self,
        id: Id,
        environment: livery::values::RelativeLengthEnvironment,
    ) {
        if let Some(computed) = self.values.get_mut(&id) {
            resolve_relative_lengths(computed, environment);
        }
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

    pub(crate) fn remove(&mut self, id: Id) {
        self.values.remove(&id);
        self.custom.remove(&id);
        self.inline_diagnostics.remove(&id);
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(Id) -> bool)
    where
        Id: Copy,
    {
        self.values.retain(|id, _| keep(*id));
        self.custom.retain(|id, _| keep(*id));
        self.inline_diagnostics.retain(|id, _| keep(*id));
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

pub(crate) fn resolve_styles_with_containers<D>(
    dom: &D,
    style_set: &StyleSet,
    device: &Device,
    states: &InteractionStates<D::NodeId>,
    containers: &HashMap<D::NodeId, Vec<ContainerSnapshot>>,
) -> StylePlane<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let selector_tree = SelectorTree::new(dom, states);
    let mut plane = StylePlane::default();
    resolve_subtree_with_containers(
        &selector_tree,
        style_set,
        device,
        dom.document(),
        None,
        None,
        &mut plane,
        Some(containers),
    );
    plane
}

pub(crate) fn resolve_subtree<D>(
    selector_tree: &SelectorTree<'_, D>,
    style_set: &StyleSet,
    device: &Device,
    id: D::NodeId,
    parent: Option<&ComputedValues>,
    parent_custom: Option<&CustomProperties>,
    plane: &mut StylePlane<D::NodeId>,
) -> usize
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    resolve_subtree_with_containers(
        selector_tree,
        style_set,
        device,
        id,
        parent,
        parent_custom,
        plane,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_subtree_with_containers<D>(
    selector_tree: &SelectorTree<'_, D>,
    style_set: &StyleSet,
    device: &Device,
    id: D::NodeId,
    parent: Option<&ComputedValues>,
    parent_custom: Option<&CustomProperties>,
    plane: &mut StylePlane<D::NodeId>,
    containers: Option<&HashMap<D::NodeId, Vec<ContainerSnapshot>>>,
) -> usize
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if selector_tree.dom().kind(id) == NodeKind::Element {
        let element = selector_tree.element(id).expect("element kind has adapter");
        let candidates = containers
            .and_then(|containers| containers.get(&id))
            .map_or(&[][..], Vec::as_slice);
        let mut matched = Vec::new();
        let mut matched_custom = Vec::new();
        for rule in &style_set.rules {
            matched.extend(rule.matched_declarations_with_containers(&element, device, candidates));
            matched_custom.extend(
                rule.matched_custom_declarations_with_containers(&element, device, candidates),
            );
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
        resolve_viewport_units(&mut computed, device);
        resolve_font_metrics(&mut computed, parent);
        let mut resolved = 1;
        for child in selector_tree.dom().dom_children(id) {
            resolved += resolve_subtree_with_containers(
                selector_tree,
                style_set,
                device,
                child,
                Some(&computed),
                Some(&custom),
                plane,
                containers,
            );
        }
        plane.values.insert(id, computed);
        plane.custom.insert(id, custom);
        resolved
    } else {
        let mut resolved = 0;
        for child in selector_tree.dom().dom_children(id) {
            resolved += resolve_subtree_with_containers(
                selector_tree,
                style_set,
                device,
                child,
                parent,
                parent_custom,
                plane,
                containers,
            );
        }
        resolved
    }
}

fn resolve_viewport_units(computed: &mut ComputedValues, device: &Device) {
    let environment = livery::values::RelativeLengthEnvironment::viewport(device.viewport_sizes)
        .with_vertical_writing(computed.writing_mode.is_vertical());
    resolve_relative_lengths(computed, environment);
}

fn resolve_relative_lengths(
    computed: &mut ComputedValues,
    environment: livery::values::RelativeLengthEnvironment,
) {
    for &property in PropertyId::ALL {
        let value = computed.get(property).resolve_relative_lengths(environment);
        computed
            .set(property, value)
            .expect("generated property read and write types agree");
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
    computed.transform.resolve_lengths(font_size, 16.0);

    if let LineHeight::Value(value) = computed.line_height {
        computed.line_height = LineHeight::Value(LengthPercentage::Length(Length::px(
            resolve_length_percentage(value, font_size, font_size).max(0.0),
        )));
    }
    for spacing in [&mut computed.letter_spacing, &mut computed.word_spacing] {
        if let livery::values::Spacing::Length(value) = *spacing {
            *spacing =
                livery::values::Spacing::Length(value.resolve_font_relative(font_size, 16.0));
        }
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
        LengthPercentage::Math(math) => {
            LengthPercentage::Math(math).to_px(em, 16.0, percentage_basis)
        },
    }
}

fn definite_transform_reference_box(values: &ComputedValues, em: f32) -> Option<(f32, f32)> {
    // Without retained layout, this CSSOM path can derive a border box only
    // for a definite, unadorned box. Paint always receives the actual fragment.
    if !box_is_unadorned(values) {
        return None;
    }
    Some((
        definite_size(values.width, em)?,
        definite_size(values.height, em)?,
    ))
}

fn box_is_unadorned(values: &ComputedValues) -> bool {
    ![
        values.padding_top,
        values.padding_right,
        values.padding_bottom,
        values.padding_left,
    ]
    .into_iter()
    .any(|padding| padding != Padding::ZERO)
        && ![
            values.border_top_style,
            values.border_right_style,
            values.border_bottom_style,
            values.border_left_style,
        ]
        .into_iter()
        .any(|style| style != BorderStyle::None)
}

fn used_px(value: f32) -> String {
    let value = (value * 10_000.0).round() / 10_000.0;
    if value == 0.0 {
        "0px".to_string()
    } else {
        Length::px(value).to_string()
    }
}

fn used_margin(
    margin: Margin,
    values: &ComputedValues,
    context: UsedValueContext,
) -> Option<f32> {
    let Margin::Value(value) = margin else {
        return None;
    };
    let basis = if value.has_percentage() {
        context.containing_inline_size?
    } else {
        0.0
    };
    let em = match values.font_size {
        FontSize::Value(LengthPercentage::Length(Length {
            value,
            unit: LengthUnit::Px,
        })) => value,
        _ => 16.0,
    };
    Some(value.to_px(em, 16.0, basis))
}

fn definite_size(size: Size, em: f32) -> Option<f32> {
    let Size::Value(value) = size else {
        return None;
    };
    (!value.has_percentage()).then(|| value.to_px(em, 16.0, 0.0))
}
