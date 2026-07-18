use std::{collections::HashMap, error::Error, fmt, hash::Hash};

use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        Alignment as CssAlignment, AspectRatio, BorderStyle, BorderWidth,
        BoxSizing as CssBoxSizing, Display as CssDisplay, FlexDirection as CssFlexDirection,
        FlexWrap as CssFlexWrap, Float as CssFloat, FontSize, Gap as CssGap,
        GridAutoFlow as CssGridAutoFlow,
        GridPlacement as CssGridPlacement, GridTemplate as CssGridTemplate,
        GridTrack as CssGridTrack, Inset, Length, LengthPercentage as CssLengthPercentage,
        LengthUnit, LineHeight, Margin, Overflow as CssOverflow, Position as CssPosition,
        Size as CssSize, WhiteSpaceCollapse,
    },
};
use taffy::{
    TaffyTree,
    geometry::{Line, Point, Rect, Size},
    prelude::{
        AvailableSpace, Dimension, LengthPercentage, LengthPercentageAuto, NodeId, auto, fr,
        length, line, max_content, min_content, percent, span,
    },
    style::{
        AlignContent, AlignContentKeyword, AlignItems, AlignItemsKeyword, BoxSizing, Display,
        Float as TaffyFloat, FlexDirection, FlexWrap, GridAutoFlow, GridPlacement,
        GridTemplateComponent,
        JustifyContent, Overflow, Position, Style,
    },
};

type ImageSources = HashMap<String, Vec<u8>>;

use crate::{StylePlane, TextSystem};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fragment {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug)]
pub struct FragmentPlane<Id> {
    fragments: HashMap<Id, Fragment>,
    atomic_fragments: HashMap<Id, Fragment>,
}

impl<Id> Default for FragmentPlane<Id> {
    fn default() -> Self {
        Self {
            fragments: HashMap::new(),
            atomic_fragments: HashMap::new(),
        }
    }
}

impl<Id: Eq + Hash> FragmentPlane<Id> {
    pub fn get(&self, id: Id) -> Option<&Fragment> {
        self.fragments.get(&id)
    }

    pub(crate) fn atomic(&self, id: Id) -> Option<&Fragment> {
        self.atomic_fragments.get(&id)
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutError(String);

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for LayoutError {}

#[derive(Clone, Debug)]
struct TextMeasure {
    width: f32,
    height: f32,
}

struct BuildState<'a, D: LayoutDom> {
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    tree: TaffyTree<TextMeasure>,
    sources: HashMap<NodeId, D::NodeId>,
    image_sources: &'a ImageSources,
}

#[derive(Clone, Debug)]
struct InlineMeasure<Id> {
    roots: Vec<Id>,
    style: ComputedValues,
    width: f32,
    height: f32,
}

struct InlineBuildState<'a, D: LayoutDom> {
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    preliminary: &'a FragmentPlane<D::NodeId>,
    tree: TaffyTree<InlineMeasure<D::NodeId>>,
    sources: HashMap<NodeId, Vec<D::NodeId>>,
    image_sources: &'a ImageSources,
}

/// Lay out a Livery style plane through a standalone Taffy tree.
///
/// This stateless entry point uses deterministic text estimates. Retained
/// Livery sessions call [`layout_with_text_system`] so Parley's shaped line
/// height participates in parent block flow.
pub fn layout<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
) -> Result<FragmentPlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let image_sources = ImageSources::new();
    layout_impl(dom, styles, viewport_width, viewport_height, &image_sources)
}

pub(crate) fn layout_with_text_system<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
    text: &mut TextSystem,
    image_sources: &ImageSources,
) -> Result<FragmentPlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let preliminary = layout_impl(dom, styles, viewport_width, viewport_height, image_sources)?;
    layout_inline_groups(
        dom,
        styles,
        viewport_width,
        viewport_height,
        text,
        &preliminary,
        image_sources,
    )
}

fn layout_impl<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
    image_sources: &ImageSources,
) -> Result<FragmentPlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut state = BuildState {
        dom,
        styles,
        tree: TaffyTree::new(),
        sources: HashMap::new(),
        image_sources,
    };
    let document = state.build_node(dom.document(), None, 16.0)?;
    let children = document.into_iter().collect::<Vec<_>>();
    let root = state
        .tree
        .new_with_children(
            Style {
                display: Display::Block,
                size: Size {
                    width: Dimension::length(viewport_width),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &children,
        )
        .map_err(taffy_error)?;

    state
        .tree
        .compute_layout_with_measure(
            root,
            Size {
                width: AvailableSpace::Definite(viewport_width),
                height: AvailableSpace::Definite(viewport_height),
            },
            |known, available, _, context, _| {
                let Some(context) = context else {
                    return Size::ZERO;
                };
                let available_width = match available.width {
                    AvailableSpace::Definite(width) => width,
                    AvailableSpace::MinContent => 0.0,
                    AvailableSpace::MaxContent => context.width,
                };
                Size {
                    width: known
                        .width
                        .unwrap_or(context.width.min(available_width.max(0.0))),
                    height: known.height.unwrap_or(context.height),
                }
            },
        )
        .map_err(taffy_error)?;

    let mut fragments = FragmentPlane::default();
    collect_fragments(
        &state.tree,
        &state.sources,
        root,
        Point { x: 0.0, y: 0.0 },
        &mut fragments,
    )?;
    Ok(fragments)
}

fn layout_inline_groups<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport_width: f32,
    viewport_height: f32,
    text: &mut TextSystem,
    preliminary: &FragmentPlane<D::NodeId>,
    image_sources: &ImageSources,
) -> Result<FragmentPlane<D::NodeId>, LayoutError>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut state = InlineBuildState {
        dom,
        styles,
        preliminary,
        tree: TaffyTree::new(),
        sources: HashMap::new(),
        image_sources,
    };
    let document = state.build_node(dom.document(), None, 16.0)?;
    let children = document.into_iter().collect::<Vec<_>>();
    let root = state
        .tree
        .new_with_children(
            Style {
                display: Display::Block,
                size: Size {
                    width: Dimension::length(viewport_width),
                    height: Dimension::auto(),
                },
                ..Style::default()
            },
            &children,
        )
        .map_err(taffy_error)?;

    state
        .tree
        .compute_layout_with_measure(
            root,
            Size {
                width: AvailableSpace::Definite(viewport_width),
                height: AvailableSpace::Definite(viewport_height),
            },
            |known, available, _, context, _| {
                let Some(context) = context else {
                    return Size::ZERO;
                };
                let available_width = match available.width {
                    AvailableSpace::Definite(width) => width,
                    AvailableSpace::MinContent => 0.0,
                    // The preliminary pass supplies a scalar estimate in
                    // `context.width`; using it for max-content would freeze
                    // the second pass at that estimate instead of asking
                    // Parley for the shaped intrinsic width.
                    AvailableSpace::MaxContent => viewport_width,
                };
                let measured = text.measure_inline_group(
                    dom,
                    styles,
                    preliminary,
                    &context.roots,
                    &context.style,
                    known.width.unwrap_or(available_width),
                );
                let (measured_width, measured_height) =
                    measured.unwrap_or((context.width, context.height));
                Size {
                    width: known.width.unwrap_or(measured_width.min(available_width.max(0.0))),
                    height: known.height.unwrap_or(measured_height),
                }
            },
        )
        .map_err(taffy_error)?;

    let mut fragments = FragmentPlane::default();
    collect_inline_fragments(
        &state.tree,
        &state.sources,
        root,
        Point { x: 0.0, y: 0.0 },
        &mut fragments,
    )?;
    for (id, fragment) in &preliminary.fragments {
        if styles.get(*id).is_some_and(|style| {
            style.display == CssDisplay::InlineBlock
                || (style.display == CssDisplay::Inline && is_replaced_element(dom, *id))
        }) {
            fragments.atomic_fragments.insert(*id, *fragment);
        }
    }
    Ok(fragments)
}

impl<D> InlineBuildState<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn build_node(
        &mut self,
        id: D::NodeId,
        inherited: Option<&ComputedValues>,
        parent_font_size: f32,
    ) -> Result<Option<NodeId>, LayoutError> {
        match self.dom.kind(id) {
            NodeKind::Document | NodeKind::DocumentFragment => {
                let child_ids = self.dom.dom_children(id).collect::<Vec<_>>();
                let children = child_ids
                    .into_iter()
                    .filter_map(|child| {
                        self.build_node(child, inherited, parent_font_size)
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if children.is_empty() {
                    Ok(None)
                } else if children.len() == 1 {
                    Ok(children.into_iter().next())
                } else {
                    self.tree
                        .new_with_children(
                            Style {
                                display: Display::Block,
                                ..Style::default()
                            },
                            &children,
                        )
                        .map(Some)
                        .map_err(taffy_error)
                }
            },
            NodeKind::Element => {
                let computed = self.styles.get(id).cloned().unwrap_or_default();
                let font_size = font_size_px(&computed.font_size, parent_font_size);
                let children = self.build_children(id, &computed, font_size)?;
                let mut taffy_style = to_taffy_style(&computed, font_size);
                apply_replaced_image_size(
                    &mut taffy_style,
                    self.dom,
                    id,
                    &computed,
                    self.image_sources,
                    font_size,
                );
                let node = self
                    .tree
                    .new_with_children(taffy_style, &children)
                    .map_err(taffy_error)?;
                self.sources.insert(node, vec![id]);
                Ok(Some(node))
            },
            NodeKind::Text => {
                let style = inherited.cloned().unwrap_or_default();
                self.build_inline_group(&[id], &style).map(Some)
            },
            _ => Ok(None),
        }
    }

    fn build_children(
        &mut self,
        parent: D::NodeId,
        parent_style: &ComputedValues,
        parent_font_size: f32,
    ) -> Result<Vec<NodeId>, LayoutError> {
        let child_ids = self.dom.dom_children(parent).collect::<Vec<_>>();
        let mut children = Vec::new();
        let mut inline_group = Vec::new();
        for child in child_ids {
            if is_inline(self.dom, self.styles, child) {
                inline_group.push(child);
                continue;
            }
            if !inline_group.is_empty() {
                children.push(self.build_inline_group(&inline_group, parent_style)?);
                inline_group.clear();
            }
            if let Some(node) = self.build_node(child, Some(parent_style), parent_font_size)? {
                children.push(node);
            }
        }
        if !inline_group.is_empty() {
            children.push(self.build_inline_group(&inline_group, parent_style)?);
        }
        Ok(children)
    }

    fn build_inline_group(
        &mut self,
        roots: &[D::NodeId],
        parent_style: &ComputedValues,
    ) -> Result<NodeId, LayoutError> {
        let width = roots
            .iter()
            .filter_map(|id| self.preliminary.get(*id))
            .map(|fragment| fragment.width)
            .sum();
        let height = roots
            .iter()
            .filter_map(|id| self.preliminary.get(*id))
            .map(|fragment| fragment.height)
            .fold(0.0_f32, f32::max);
        let node = self
            .tree
            .new_leaf_with_context(
                Style {
                    display: Display::Block,
                    ..Style::default()
                },
                InlineMeasure {
                    roots: roots.to_vec(),
                    style: parent_style.clone(),
                    width,
                    height,
                },
            )
            .map_err(taffy_error)?;
        self.sources.insert(node, roots.to_vec());
        Ok(node)
    }
}

impl<D> BuildState<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn build_node(
        &mut self,
        id: D::NodeId,
        inherited: Option<&ComputedValues>,
        parent_font_size: f32,
    ) -> Result<Option<NodeId>, LayoutError> {
        match self.dom.kind(id) {
            NodeKind::Document | NodeKind::DocumentFragment => {
                let children = self
                    .dom
                    .dom_children(id)
                    .filter_map(|child| {
                        self.build_node(child, inherited, parent_font_size)
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if children.is_empty() {
                    Ok(None)
                } else if children.len() == 1 {
                    Ok(children.into_iter().next())
                } else {
                    self.tree
                        .new_with_children(
                            Style {
                                display: Display::Block,
                                ..Style::default()
                            },
                            &children,
                        )
                        .map(Some)
                        .map_err(taffy_error)
                }
            },
            NodeKind::Element => {
                let computed = self.styles.get(id).cloned().unwrap_or_default();
                let font_size = font_size_px(&computed.font_size, parent_font_size);
                let children = self
                    .dom
                    .dom_children(id)
                    .filter_map(|child| {
                        self.build_node(child, Some(&computed), font_size)
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut taffy_style = to_taffy_style(&computed, font_size);
                apply_replaced_image_size(
                    &mut taffy_style,
                    self.dom,
                    id,
                    &computed,
                    self.image_sources,
                    font_size,
                );
                let node = self
                    .tree
                    .new_with_children(taffy_style, &children)
                    .map_err(taffy_error)?;
                self.sources.insert(node, id);
                Ok(Some(node))
            },
            NodeKind::Text => {
                let text = self.dom.text(id).unwrap_or("");
                let preserves_whitespace = inherited.is_some_and(|style| {
                    matches!(
                        style.white_space_collapse,
                        WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
                    )
                });
                if text.is_empty() || (!preserves_whitespace && text.trim().is_empty()) {
                    return Ok(None);
                }
                let font_size = parent_font_size;
                let line_height = inherited
                    .map(|style| line_height_px(&style.line_height, font_size))
                    .unwrap_or(font_size * 1.2);
                let width = text
                    .lines()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(0) as f32
                    * font_size
                    * 0.6;
                let height = text.lines().count().max(1) as f32 * line_height;
                let node = self
                    .tree
                    .new_leaf_with_context(
                        Style {
                            display: Display::Block,
                            ..Style::default()
                        },
                        TextMeasure { width, height },
                    )
                    .map_err(taffy_error)?;
                self.sources.insert(node, id);
                Ok(Some(node))
            },
            _ => Ok(None),
        }
    }
}

fn collect_fragments<Id>(
    tree: &TaffyTree<TextMeasure>,
    sources: &HashMap<NodeId, Id>,
    node: NodeId,
    parent_origin: Point<f32>,
    fragments: &mut FragmentPlane<Id>,
) -> Result<(), LayoutError>
where
    Id: Copy + Eq + Hash,
{
    let computed = tree.layout(node).map_err(taffy_error)?;
    let origin = Point {
        x: parent_origin.x + computed.location.x,
        y: parent_origin.y + computed.location.y,
    };
    if let Some(source) = sources.get(&node) {
        fragments.fragments.insert(
            *source,
            Fragment {
                x: origin.x,
                y: origin.y,
                width: computed.size.width,
                height: computed.size.height,
            },
        );
    }
    for child in tree.children(node).map_err(taffy_error)? {
        collect_fragments(tree, sources, child, origin, fragments)?;
    }
    Ok(())
}

fn collect_inline_fragments<Id>(
    tree: &TaffyTree<InlineMeasure<Id>>,
    sources: &HashMap<NodeId, Vec<Id>>,
    node: NodeId,
    parent_origin: Point<f32>,
    fragments: &mut FragmentPlane<Id>,
) -> Result<(), LayoutError>
where
    Id: Copy + Eq + Hash,
{
    let computed = tree.layout(node).map_err(taffy_error)?;
    let origin = Point {
        x: parent_origin.x + computed.location.x,
        y: parent_origin.y + computed.location.y,
    };
    if let Some(source_ids) = sources.get(&node) {
        let fragment = Fragment {
            x: origin.x,
            y: origin.y,
            width: computed.size.width,
            height: computed.size.height,
        };
        for source in source_ids {
            fragments.fragments.insert(*source, fragment);
        }
    }
    for child in tree.children(node).map_err(taffy_error)? {
        collect_inline_fragments(tree, sources, child, origin, fragments)?;
    }
    Ok(())
}

fn is_inline<D>(dom: &D, styles: &StylePlane<D::NodeId>, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    match dom.kind(id) {
        NodeKind::Text => true,
        NodeKind::Element => styles.get(id).is_some_and(|style| {
            matches!(style.display, CssDisplay::Inline | CssDisplay::InlineBlock)
        }),
        _ => false,
    }
}

/// Return the topmost pointer-events-enabled element whose layout fragment
/// contains a scene point. The walk mirrors the lane's DOM paint order for the
/// bounded stacking subset: numeric z-index first, then source order.
pub fn hit_test<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    x: f32,
    y: f32,
) -> Option<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    hit_test_with_scroll(dom, styles, fragments, &HashMap::new(), x, y)
}

/// Hit-test a retained fragment plane after applying per-element scroll
/// offsets to descendants. The ordinary [`hit_test`] path keeps the map empty;
/// retained sessions use this variant for wheel-scrolled containers.
pub(crate) fn hit_test_with_scroll<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    scroll_offsets: &HashMap<D::NodeId, (f32, f32)>,
    x: f32,
    y: f32,
) -> Option<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut state = HitTestState {
        dom,
        styles,
        fragments,
        scroll_offsets,
        x,
        y,
        clips: Vec::new(),
        order: 0,
        candidates: Vec::new(),
    };
    collect_hit_candidates(&mut state, dom.document(), (0.0, 0.0));
    state
        .candidates
        .into_iter()
        .max_by_key(|candidate| (candidate.level, candidate.order))
        .map(|candidate| candidate.id)
}

struct HitCandidate<Id> {
    id: Id,
    level: i32,
    order: u64,
}

struct HitTestState<'a, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &'a FragmentPlane<D::NodeId>,
    scroll_offsets: &'a HashMap<D::NodeId, (f32, f32)>,
    x: f32,
    y: f32,
    clips: Vec<(f32, f32, f32, f32)>,
    order: u64,
    candidates: Vec<HitCandidate<D::NodeId>>,
}

fn collect_hit_candidates<D>(
    state: &mut HitTestState<'_, D>,
    id: D::NodeId,
    ancestor_scroll: (f32, f32),
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let style = state.styles.get(id);
    let fragment = state.fragments.get(id);
    let visible_fragment = fragment.map(|fragment| Fragment {
        x: fragment.x - ancestor_scroll.0,
        y: fragment.y - ancestor_scroll.1,
        ..*fragment
    });
    let inside_clips = state.clips.iter().all(|(left, top, right, bottom)| {
        state.x >= *left && state.x <= *right && state.y >= *top && state.y <= *bottom
    });
    if state.dom.kind(id) == NodeKind::Element
        && let (Some(style), Some(fragment)) = (style, visible_fragment)
        && style.display != CssDisplay::None
        && style.visibility == livery::values::Visibility::Visible
        && style.pointer_events == livery::values::PointerEvents::Auto
        && inside_clips
        && state.x >= fragment.x
        && state.x <= fragment.x + fragment.width
        && state.y >= fragment.y
        && state.y <= fragment.y + fragment.height
    {
        let level = match style.z_index {
            livery::values::ZIndex::Integer(level) => level,
            livery::values::ZIndex::Auto => 0,
        };
        state.candidates.push(HitCandidate {
            id,
            level,
            order: state.order,
        });
    }
    state.order = state.order.saturating_add(1);

    let pushed_clip = style
        .zip(visible_fragment)
        .filter(|(style, _)| {
            style.overflow_x != CssOverflow::Visible || style.overflow_y != CssOverflow::Visible
        })
        .map(|(_, fragment)| {
            (
                fragment.x,
                fragment.y,
                fragment.x + fragment.width,
                fragment.y + fragment.height,
            )
        });
    if let Some(clip) = pushed_clip.as_ref() {
        state.clips.push(*clip);
    }
    let children = state.dom.dom_children(id).collect::<Vec<_>>();
    let next_scroll = state
        .scroll_offsets
        .get(&id)
        .copied()
        .map_or(ancestor_scroll, |offset| {
            (ancestor_scroll.0 + offset.0, ancestor_scroll.1 + offset.1)
        });
    for child in children {
        collect_hit_candidates(state, child, next_scroll);
    }
    if pushed_clip.is_some() {
        state.clips.pop();
    }
}

fn is_replaced_element<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    dom.kind(id) == NodeKind::Element
        && dom
            .element_name(id)
            .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("img"))
}

fn apply_replaced_image_size<D>(
    style: &mut Style,
    dom: &D,
    id: D::NodeId,
    computed: &ComputedValues,
    image_sources: &ImageSources,
    font_size: f32,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let intrinsic = image_intrinsic_size(dom, id, image_sources).filter(|(width, height)| {
        *width > 0.0 && *height > 0.0
    });

    // HTML width/height attributes are presentational hints for replaced
    // elements.  A definite CSS declaration wins, while an attribute fills
    // an otherwise-auto dimension before the intrinsic ratio is applied.
    let width = definite_size(computed.width, font_size)
        .or_else(|| image_attribute_size(dom, id, "width"));
    let height = definite_size(computed.height, font_size)
        .or_else(|| image_attribute_size(dom, id, "height"));
    let width = width.filter(|value| *value > 0.0);
    let height = height.filter(|value| *value > 0.0);
    if let Some((intrinsic_width, intrinsic_height)) = intrinsic
        && style.aspect_ratio.is_none()
        && !(width.is_some() && height.is_some())
    {
        style.aspect_ratio = Some(intrinsic_width / intrinsic_height);
    }
    match (width, height, intrinsic) {
        (Some(width), Some(height), _) => {
            style.size.width = Dimension::length(width);
            style.size.height = Dimension::length(height);
        },
        (Some(width), None, Some((intrinsic_width, intrinsic_height))) => {
            style.size.width = Dimension::length(width);
            style.size.height = Dimension::length(width * intrinsic_height / intrinsic_width);
        },
        (Some(width), None, None) => {
            style.size.width = Dimension::length(width);
        },
        (None, Some(height), Some((intrinsic_width, intrinsic_height))) => {
            style.size.width = Dimension::length(height * intrinsic_width / intrinsic_height);
            style.size.height = Dimension::length(height);
        },
        (None, Some(height), None) => {
            style.size.height = Dimension::length(height);
        },
        (None, None, Some((intrinsic_width, intrinsic_height))) => {
            style.size.width = Dimension::length(intrinsic_width);
            style.size.height = Dimension::length(intrinsic_height);
        },
        (None, None, None) => {},
    }
}

fn image_attribute_size<D>(dom: &D, id: D::NodeId, name: &str) -> Option<f32>
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    dom.attributes(id).find_map(|attribute| {
        (attribute.name.ns.as_ref().is_empty()
            && attribute.name.local.as_ref().eq_ignore_ascii_case(name))
            .then(|| attribute.value.trim().parse::<f32>().ok())
            .flatten()
            .filter(|value| value.is_finite() && *value > 0.0)
    })
}

fn image_intrinsic_size<D>(
    dom: &D,
    id: D::NodeId,
    image_sources: &ImageSources,
) -> Option<(f32, f32)>
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    if dom.kind(id) != NodeKind::Element
        || !dom
            .element_name(id)
            .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("img"))
    {
        return None;
    }
    let source = dom.attributes(id).find_map(|attribute| {
        (attribute.name.ns.as_ref().is_empty()
            && attribute.name.local.as_ref().eq_ignore_ascii_case("src"))
        .then_some(attribute.value)
    })?;
    let bytes = if let Ok(data_url) = data_url::DataUrl::process(source) {
        data_url.decode_to_vec().ok()?.0
    } else {
        image_sources.get(source)?.clone()
    };
    let image = image::load_from_memory(&bytes).ok()?;
    Some((image.width() as f32, image.height() as f32))
}

fn definite_size(size: CssSize, font_size: f32) -> Option<f32> {
    let CssSize::Value(value) = size else {
        return None;
    };
    match value {
        CssLengthPercentage::Length(length) => Some(absolute_length(length, font_size, 16.0)),
        CssLengthPercentage::Calc(calc) if calc.percentage == 0.0 => {
            Some(calc.px + calc.em * font_size + calc.rem * 16.0)
        },
        _ => None,
    }
}

fn to_taffy_style(computed: &ComputedValues, font_size: f32) -> Style {
    Style {
        display: match computed.display {
            CssDisplay::None => Display::None,
            CssDisplay::Flex => Display::Flex,
            CssDisplay::Grid => Display::Grid,
            _ => Display::Block,
        },
        float: match computed.float {
            CssFloat::None => TaffyFloat::None,
            CssFloat::Left => TaffyFloat::Left,
            CssFloat::Right => TaffyFloat::Right,
        },
        box_sizing: match computed.box_sizing {
            CssBoxSizing::ContentBox => BoxSizing::ContentBox,
            CssBoxSizing::BorderBox => BoxSizing::BorderBox,
        },
        overflow: Point {
            x: overflow(computed.overflow_x),
            y: overflow(computed.overflow_y),
        },
        position: match computed.position {
            CssPosition::Absolute | CssPosition::Fixed => Position::Absolute,
            _ => Position::Relative,
        },
        inset: if matches!(computed.position, CssPosition::Static) {
            Rect::auto()
        } else {
            Rect {
                left: inset(computed.left, font_size),
                right: inset(computed.right, font_size),
                top: inset(computed.top, font_size),
                bottom: inset(computed.bottom, font_size),
            }
        },
        size: Size {
            width: dimension(computed.width, font_size),
            height: dimension(computed.height, font_size),
        },
        min_size: Size {
            width: dimension(computed.min_width, font_size),
            height: dimension(computed.min_height, font_size),
        },
        max_size: Size {
            width: dimension(computed.max_width, font_size),
            height: dimension(computed.max_height, font_size),
        },
        aspect_ratio: match computed.aspect_ratio {
            AspectRatio::Auto => None,
            AspectRatio::Ratio(value) => Some(value),
        },
        flex_direction: match computed.flex_direction {
            CssFlexDirection::Row => FlexDirection::Row,
            CssFlexDirection::RowReverse => FlexDirection::RowReverse,
            CssFlexDirection::Column => FlexDirection::Column,
            CssFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
        },
        flex_wrap: match computed.flex_wrap {
            CssFlexWrap::NoWrap => FlexWrap::NoWrap,
            CssFlexWrap::Wrap => FlexWrap::Wrap,
            CssFlexWrap::WrapReverse => FlexWrap::WrapReverse,
        },
        flex_basis: dimension(computed.flex_basis, font_size),
        flex_grow: computed.flex_grow.value(),
        flex_shrink: computed.flex_shrink.value(),
        order: computed.order.value(),
        margin: Rect {
            left: margin(computed.margin_left, font_size),
            right: margin(computed.margin_right, font_size),
            top: margin(computed.margin_top, font_size),
            bottom: margin(computed.margin_bottom, font_size),
        },
        padding: Rect {
            left: length_percentage(computed.padding_left.0, font_size),
            right: length_percentage(computed.padding_right.0, font_size),
            top: length_percentage(computed.padding_top.0, font_size),
            bottom: length_percentage(computed.padding_bottom.0, font_size),
        },
        border: Rect {
            left: border(
                computed.border_left_style,
                computed.border_left_width,
                font_size,
            ),
            right: border(
                computed.border_right_style,
                computed.border_right_width,
                font_size,
            ),
            top: border(
                computed.border_top_style,
                computed.border_top_width,
                font_size,
            ),
            bottom: border(
                computed.border_bottom_style,
                computed.border_bottom_width,
                font_size,
            ),
        },
        gap: Size {
            width: gap(computed.column_gap, font_size),
            height: gap(computed.row_gap, font_size),
        },
        align_items: Some(align_items(computed.align_items)),
        align_content: Some(align_content(computed.align_content)),
        justify_content: Some(justify_content(computed.justify_content)),
        grid_template_columns: grid_template(&computed.grid_template_columns, font_size),
        grid_template_rows: grid_template(&computed.grid_template_rows, font_size),
        grid_auto_flow: grid_auto_flow(computed.grid_auto_flow),
        grid_column: Line {
            start: grid_placement(computed.grid_column_start),
            end: grid_placement(computed.grid_column_end),
        },
        grid_row: Line {
            start: grid_placement(computed.grid_row_start),
            end: grid_placement(computed.grid_row_end),
        },
        ..Style::default()
    }
}

fn grid_auto_flow(value: CssGridAutoFlow) -> GridAutoFlow {
    match value {
        CssGridAutoFlow::Row => GridAutoFlow::Row,
        CssGridAutoFlow::Column => GridAutoFlow::Column,
        CssGridAutoFlow::RowDense => GridAutoFlow::RowDense,
        CssGridAutoFlow::ColumnDense => GridAutoFlow::ColumnDense,
    }
}

fn grid_placement(value: CssGridPlacement) -> GridPlacement {
    match value {
        CssGridPlacement::Auto => GridPlacement::Auto,
        CssGridPlacement::Line(value) => line(value),
        CssGridPlacement::Span(value) => span(value),
    }
}

fn grid_template(value: &CssGridTemplate, _em: f32) -> Vec<GridTemplateComponent<String>> {
    match value {
        CssGridTemplate::None => Vec::new(),
        CssGridTemplate::Tracks(tracks) => tracks
            .iter()
            .map(|track| match track {
                CssGridTrack::Auto => auto(),
                CssGridTrack::MinContent => min_content(),
                CssGridTrack::MaxContent => max_content(),
                CssGridTrack::Px(value) => length(*value),
                CssGridTrack::Percent(value) => percent(*value),
                CssGridTrack::Fr(value) => fr(*value),
            })
            .collect(),
    }
}

fn align_items(value: CssAlignment) -> AlignItems {
    AlignItems {
        keyword: match value {
            CssAlignment::Start => AlignItemsKeyword::Start,
            CssAlignment::End => AlignItemsKeyword::End,
            CssAlignment::FlexStart => AlignItemsKeyword::FlexStart,
            CssAlignment::FlexEnd => AlignItemsKeyword::FlexEnd,
            CssAlignment::Center => AlignItemsKeyword::Center,
            CssAlignment::Baseline => AlignItemsKeyword::Baseline,
            _ => AlignItemsKeyword::Stretch,
        },
        safety: taffy::style::AlignmentSafety::Unsafe,
    }
}

fn align_content(value: CssAlignment) -> AlignContent {
    AlignContent {
        keyword: match value {
            CssAlignment::Start => AlignContentKeyword::Start,
            CssAlignment::End => AlignContentKeyword::End,
            CssAlignment::FlexStart => AlignContentKeyword::FlexStart,
            CssAlignment::FlexEnd => AlignContentKeyword::FlexEnd,
            CssAlignment::Center => AlignContentKeyword::Center,
            CssAlignment::SpaceBetween => AlignContentKeyword::SpaceBetween,
            CssAlignment::SpaceAround => AlignContentKeyword::SpaceAround,
            CssAlignment::SpaceEvenly => AlignContentKeyword::SpaceEvenly,
            _ => AlignContentKeyword::Stretch,
        },
        safety: taffy::style::AlignmentSafety::Unsafe,
    }
}

fn justify_content(value: CssAlignment) -> JustifyContent {
    align_content(value)
}

fn font_size_px(size: &FontSize, parent: f32) -> f32 {
    match size {
        FontSize::Medium => 16.0,
        FontSize::Value(value) => absolute_length_percentage(*value, parent, 16.0, parent),
    }
    .max(0.0)
}

pub(crate) fn line_height_px(height: &LineHeight, font_size: f32) -> f32 {
    match height {
        LineHeight::Normal => font_size * 1.2,
        LineHeight::Number(value) => font_size * value,
        LineHeight::Value(value) => absolute_length_percentage(*value, font_size, 16.0, font_size),
    }
}

fn dimension(size: CssSize, em: f32) -> Dimension {
    match size {
        CssSize::Value(value) => match value {
            CssLengthPercentage::Percentage(value) => Dimension::percent(value),
            _ => Dimension::length(absolute_length_percentage(value, em, 16.0, 0.0)),
        },
        _ => Dimension::auto(),
    }
}

fn inset(value: Inset, em: f32) -> LengthPercentageAuto {
    match value {
        Inset::Auto => LengthPercentageAuto::auto(),
        Inset::Value(value) => length_percentage_auto(value, em),
    }
}

fn margin(value: Margin, em: f32) -> LengthPercentageAuto {
    match value {
        Margin::Auto => LengthPercentageAuto::auto(),
        Margin::Value(value) => length_percentage_auto(value, em),
    }
}

fn length_percentage_auto(value: CssLengthPercentage, em: f32) -> LengthPercentageAuto {
    match value {
        CssLengthPercentage::Percentage(value) => LengthPercentageAuto::percent(value),
        _ => LengthPercentageAuto::length(absolute_length_percentage(value, em, 16.0, 0.0)),
    }
}

fn length_percentage(value: CssLengthPercentage, em: f32) -> LengthPercentage {
    match value {
        CssLengthPercentage::Percentage(value) => LengthPercentage::percent(value),
        _ => LengthPercentage::length(absolute_length_percentage(value, em, 16.0, 0.0)),
    }
}

fn gap(value: CssGap, em: f32) -> LengthPercentage {
    length_percentage(value.0, em)
}

fn absolute_length_percentage(
    value: CssLengthPercentage,
    em: f32,
    rem: f32,
    percentage_basis: f32,
) -> f32 {
    match value {
        CssLengthPercentage::Zero => 0.0,
        CssLengthPercentage::Length(length) => absolute_length(length, em, rem),
        CssLengthPercentage::Percentage(value) => percentage_basis * value,
        CssLengthPercentage::Calc(calc) => {
            percentage_basis * calc.percentage + calc.px + calc.em * em + calc.rem * rem
        },
    }
}

pub(crate) fn length_percentage_px(
    value: CssLengthPercentage,
    em: f32,
    percentage_basis: f32,
) -> f32 {
    absolute_length_percentage(value, em, 16.0, percentage_basis).max(0.0)
}

pub(crate) fn signed_length_percentage_px(
    value: CssLengthPercentage,
    em: f32,
    percentage_basis: f32,
) -> f32 {
    absolute_length_percentage(value, em, 16.0, percentage_basis)
}

fn absolute_length(length: Length, em: f32, rem: f32) -> f32 {
    length.unit.to_px(length.value, em, rem)
}

pub(crate) fn border_width_px(style: BorderStyle, width: BorderWidth, em: f32) -> f32 {
    if matches!(style, BorderStyle::None | BorderStyle::Hidden) {
        return 0.0;
    }
    match width {
        BorderWidth::Thin => 1.0,
        BorderWidth::Medium => 3.0,
        BorderWidth::Thick => 5.0,
        BorderWidth::Length(length) => absolute_length(length, em, 16.0),
    }
    .max(0.0)
}

fn border(style: BorderStyle, width: BorderWidth, em: f32) -> LengthPercentage {
    LengthPercentage::length(border_width_px(style, width, em))
}

fn overflow(value: CssOverflow) -> Overflow {
    match value {
        CssOverflow::Visible => Overflow::Visible,
        CssOverflow::Hidden => Overflow::Hidden,
        CssOverflow::Clip => Overflow::Clip,
        CssOverflow::Scroll | CssOverflow::Auto => Overflow::Scroll,
    }
}

fn taffy_error(error: impl fmt::Debug) -> LayoutError {
    LayoutError(format!("Taffy layout error: {error:?}"))
}
