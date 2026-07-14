//! Clean-room Parley adapter for Livery inline formatting.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    hash::Hash,
    ops::Range,
    sync::Arc,
};

use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        Display, FontFamily as CssFontFamily, FontStyle as CssFontStyle,
        FontWeight as CssFontWeight, LineHeight as CssLineHeight, TextWrapMode,
    },
};
use paint_list_api::{
    ColorF, CommonPlacement, FontInstanceKey, FontResource, GlyphInstance, IdNamespace,
    LayoutPoint, PaintCmd, TextOptions, TextRunItem,
};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, GenericFamily,
    InlineBox, InlineBoxKind, LayoutContext, PositionedLayoutItem, StyleProperty,
};

use crate::{Fragment, FragmentPlane, StylePlane, paint::resolve_color};

#[derive(Clone, Debug, Default, PartialEq)]
struct Brush {
    color: [f32; 4],
    source_index: usize,
}

/// Retained font discovery, shaping scratch space, and font resources for one
/// Livery document session.
pub struct TextSystem {
    font_context: FontContext,
    layout_context: LayoutContext<Brush>,
    fonts: HashMap<FontInstanceKey, FontResource>,
    font_keys: HashMap<(u64, u32), FontInstanceKey>,
    shape_count: u64,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl TextSystem {
    pub fn new() -> Self {
        Self {
            font_context: FontContext::new(),
            layout_context: LayoutContext::new(),
            fonts: HashMap::new(),
            font_keys: HashMap::new(),
            shape_count: 0,
        }
    }

    pub fn shape_count(&self) -> u64 {
        self.shape_count
    }

    pub fn retained_font_count(&self) -> usize {
        self.fonts.len()
    }

    /// Measure one consecutive inline group with the same collection, styles,
    /// atomic boxes, and line breaking used by paint.
    pub(crate) fn measure_inline_group<D>(
        &mut self,
        dom: &D,
        styles: &StylePlane<D::NodeId>,
        fragments: &FragmentPlane<D::NodeId>,
        roots: &[D::NodeId],
        parent_style: &ComputedValues,
        width: f32,
    ) -> (f32, f32)
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut inline_boxes = Vec::new();
        let mut owners = Vec::new();
        let already_prepared = HashSet::new();
        {
            let mut collector = InlineCollector {
                dom,
                styles,
                fragments,
                already_prepared: &already_prepared,
                owners: &mut owners,
                text: &mut text,
                spans: &mut spans,
                inline_boxes: &mut inline_boxes,
                percentage_basis: width,
            };
            for root in roots {
                collector.collect(*root, parent_style);
            }
        }
        if spans.is_empty() && inline_boxes.is_empty() {
            return (0.0, 0.0);
        }

        let items = self.shape(&text, &mut spans, &inline_boxes, width, parent_style);
        let mut right = 0.0_f32;
        let mut top = f32::INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        for item in items {
            let fragment = match item {
                ShapedItem::Text(run) => run.fragment,
                ShapedItem::InlineBox { fragment, .. } => fragment,
            };
            right = right.max(fragment.x + fragment.width);
            top = top.min(fragment.y);
            bottom = bottom.max(fragment.y + fragment.height);
        }
        if top.is_finite() && bottom.is_finite() {
            (right.max(0.0), (bottom - top).max(0.0))
        } else {
            (0.0, 0.0)
        }
    }

    pub(crate) fn begin_frame<Id>(&self) -> TextFrame<Id> {
        TextFrame::default()
    }

    pub(crate) fn fonts_for<Id>(&self, frame: &TextFrame<Id>) -> Vec<FontResource>
    where
        Id: Eq + Hash,
    {
        let mut fonts = frame
            .used_fonts
            .iter()
            .filter_map(|key| self.fonts.get(key).cloned())
            .collect::<Vec<_>>();
        fonts.sort_by_key(|font| (font.key.0.0, font.key.1));
        fonts
    }

    /// Shape each consecutive inline child group into one Parley layout. The
    /// glyph runs stay keyed by their source text node so the DOM paint walk
    /// keeps source order while line breaking and baselines are shared.
    pub(crate) fn prepare_inline_children<D>(
        &mut self,
        frame: &mut TextFrame<D::NodeId>,
        dom: &D,
        styles: &StylePlane<D::NodeId>,
        fragments: &FragmentPlane<D::NodeId>,
        parent: D::NodeId,
        parent_style: &ComputedValues,
    ) where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        let Some(parent_fragment) = frame
            .inline_fragments(parent)
            .and_then(|fragments| fragments.first())
            .or_else(|| fragments.get(parent))
            .copied()
        else {
            return;
        };
        let mut group = Vec::new();
        for child in dom.dom_children(parent) {
            if is_inline(dom, styles, child) {
                group.push(child);
            } else {
                self.flush_group(
                    frame,
                    dom,
                    styles,
                    fragments,
                    &group,
                    (&parent_fragment, parent_style),
                );
                group.clear();
            }
        }
        self.flush_group(
            frame,
            dom,
            styles,
            fragments,
            &group,
            (&parent_fragment, parent_style),
        );
    }

    pub(crate) fn emit_single<Id>(
        &mut self,
        frame: &mut TextFrame<Id>,
        source: &str,
        style: &ComputedValues,
        fragment: &Fragment,
        commands: &mut Vec<PaintCmd>,
    ) where
        Id: Copy + Eq + Hash,
    {
        let text = normalized_text(source, style);
        if text.is_empty() {
            return;
        }
        let mut spans = vec![SourceSpan::<Id> {
            source: None,
            owners: Vec::new(),
            style: style.clone(),
            range: 0..text.len(),
        }];
        for item in self.shape(text.as_ref(), &mut spans, &[], fragment.width, style) {
            let ShapedItem::Text(mut run) = item else {
                continue;
            };
            for glyph in &mut run.glyphs {
                glyph.point.x += fragment.x;
                glyph.point.y += fragment.y;
            }
            frame.used_fonts.insert(run.font_instance);
            commands.push(PaintCmd::DrawText(TextRunItem {
                placement: CommonPlacement::new(super::paint::bounds(fragment)),
                font_instance: run.font_instance,
                font_size: run.font_size,
                color: run.color,
                glyphs: run.glyphs,
                options: TextOptions::default(),
            }));
        }
    }

    fn flush_group<D>(
        &mut self,
        frame: &mut TextFrame<D::NodeId>,
        dom: &D,
        styles: &StylePlane<D::NodeId>,
        fragments: &FragmentPlane<D::NodeId>,
        roots: &[D::NodeId],
        parent: (&Fragment, &ComputedValues),
    ) where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        if roots.is_empty() {
            return;
        }
        let (parent_fragment, parent_style) = parent;
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut inline_boxes = Vec::new();
        let mut owners = Vec::new();
        {
            let mut collector = InlineCollector {
                dom,
                styles,
                fragments,
                already_prepared: &frame.prepared_sources,
                owners: &mut owners,
                text: &mut text,
                spans: &mut spans,
                inline_boxes: &mut inline_boxes,
                percentage_basis: parent_fragment.width,
            };
            for root in roots {
                collector.collect(*root, parent_style);
            }
        }
        if spans.is_empty() && inline_boxes.is_empty() {
            return;
        }

        let origin = spans
            .iter()
            .filter_map(|span| span.source.and_then(|id| fragments.get(id)))
            .next()
            .or_else(|| roots.iter().find_map(|id| fragments.get(*id)))
            .map_or((parent_fragment.x, parent_fragment.y), |fragment| {
                (fragment.x, fragment.y)
            });
        for item in self.shape(
            &text,
            &mut spans,
            &inline_boxes,
            parent_fragment.width,
            parent_style,
        ) {
            match item {
                ShapedItem::Text(mut run) => {
                    let Some(source) = run.source else {
                        continue;
                    };
                    translate_fragment(&mut run.fragment, origin);
                    let line_y = run.fragment.y;
                    for glyph in &mut run.glyphs {
                        glyph.point.x += origin.0;
                        glyph.point.y += origin.1;
                    }
                    frame.record_inline_fragment(source, run.fragment, line_y);
                    for owner in run.owners {
                        frame.record_inline_fragment(
                            owner,
                            decorated_inline_fragment(
                                styles,
                                owner,
                                run.fragment,
                                parent_fragment.width,
                            ),
                            line_y,
                        );
                    }
                    let command = PaintCmd::DrawText(TextRunItem {
                        placement: CommonPlacement::new(super::paint::bounds(parent_fragment)),
                        font_instance: run.font_instance,
                        font_size: run.font_size,
                        color: run.color,
                        glyphs: run.glyphs,
                        options: TextOptions::default(),
                    });
                    frame.used_fonts.insert(run.font_instance);
                    frame.prepared_sources.insert(source);
                    frame.prepared.entry(source).or_default().push(command);
                },
                ShapedItem::InlineBox {
                    source,
                    owners,
                    mut fragment,
                    edge,
                    mut line_y,
                } => {
                    translate_fragment(&mut fragment, origin);
                    line_y += origin.1;
                    frame.record_inline_fragment(
                        source,
                        if edge {
                            decorated_inline_fragment(
                                styles,
                                source,
                                fragment,
                                parent_fragment.width,
                            )
                        } else {
                            fragment
                        },
                        line_y,
                    );
                    for owner in owners {
                        frame.record_inline_fragment(
                            owner,
                            decorated_inline_fragment(
                                styles,
                                owner,
                                fragment,
                                parent_fragment.width,
                            ),
                            line_y,
                        );
                    }
                },
            }
        }
    }

    fn shape<Id>(
        &mut self,
        text: &str,
        spans: &mut [SourceSpan<Id>],
        inline_boxes: &[InlineAtom<Id>],
        width: f32,
        root_style: &ComputedValues,
    ) -> Vec<ShapedItem<Id>>
    where
        Id: Copy,
    {
        self.shape_count = self.shape_count.saturating_add(1);
        let mut builder =
            self.layout_context
                .ranged_builder(&mut self.font_context, text, 1.0, true);
        push_defaults(
            &mut builder,
            spans.first().map_or(root_style, |span| &span.style),
        );
        for (source_index, span) in spans.iter().enumerate() {
            push_span(&mut builder, &span.style, span.range.clone(), source_index);
        }
        for (index, inline_box) in inline_boxes.iter().enumerate() {
            builder.push_inline_box(InlineBox {
                id: u64::try_from(index).unwrap_or(u64::MAX),
                kind: InlineBoxKind::InFlow,
                index: inline_box.index,
                width: inline_box.fragment.width,
                height: if inline_box.edge {
                    0.0
                } else {
                    inline_box.fragment.height
                },
            });
        }
        let mut layout = builder.build(text);
        let wrap_width = (root_style.text_wrap_mode == TextWrapMode::Wrap)
            .then_some(width)
            .filter(|width| width.is_finite() && *width > 0.0);
        layout.break_all_lines(wrap_width);
        layout.align(Alignment::Start, AlignmentOptions::default());

        let mut result = Vec::new();
        for line in layout.lines() {
            let metrics = *line.metrics();
            for item in line.items() {
                match item {
                    PositionedLayoutItem::GlyphRun(run) => {
                        let parley_run = run.run();
                        let brush = &run.style().brush;
                        let span = spans.get(brush.source_index);
                        let glyphs = run
                            .positioned_glyphs()
                            .map(|glyph| GlyphInstance {
                                index: glyph.id,
                                point: LayoutPoint::new(glyph.x, glyph.y),
                            })
                            .collect::<Vec<_>>();
                        if glyphs.is_empty() {
                            continue;
                        }
                        let [red, green, blue, alpha] = brush.color;
                        result.push(ShapedItem::Text(ShapedRun {
                            source: span.and_then(|span| span.source),
                            owners: span.map_or_else(Vec::new, |span| span.owners.clone()),
                            fragment: Fragment {
                                x: run.offset(),
                                y: metrics.block_min_coord,
                                width: run.advance().max(0.0),
                                height: (metrics.block_max_coord - metrics.block_min_coord)
                                    .max(0.0),
                            },
                            font_instance: self.intern_font(parley_run.font()),
                            font_size: parley_run.font_size(),
                            color: ColorF::new(red, green, blue, alpha),
                            glyphs,
                        }));
                    },
                    PositionedLayoutItem::InlineBox(positioned) => {
                        let Some(inline_box) = usize::try_from(positioned.id)
                            .ok()
                            .and_then(|index| inline_boxes.get(index))
                        else {
                            continue;
                        };
                        result.push(ShapedItem::InlineBox {
                            source: inline_box.source,
                            owners: inline_box.owners.clone(),
                            fragment: Fragment {
                                x: positioned.x,
                                y: if inline_box.edge {
                                    metrics.block_min_coord
                                } else {
                                    positioned.y
                                },
                                width: positioned.width,
                                height: if inline_box.edge {
                                    (metrics.block_max_coord - metrics.block_min_coord).max(0.0)
                                } else {
                                    positioned.height
                                },
                            },
                            edge: inline_box.edge,
                            line_y: metrics.block_min_coord,
                        });
                    },
                }
            }
        }
        result
    }

    fn intern_font(&mut self, font: &parley::FontData) -> FontInstanceKey {
        let identity = (font.data.id(), font.index);
        if let Some(key) = self.font_keys.get(&identity) {
            return *key;
        }
        let bytes = font.data.data();
        let key = content_key(bytes, font.index);
        self.fonts.entry(key).or_insert_with(|| FontResource {
            key,
            data: Arc::new(bytes.to_vec()),
            index: font.index,
        });
        self.font_keys.insert(identity, key);
        key
    }
}

pub(crate) struct TextFrame<Id> {
    prepared: HashMap<Id, Vec<PaintCmd>>,
    prepared_sources: HashSet<Id>,
    inline_fragments: HashMap<Id, Vec<Fragment>>,
    inline_line_keys: HashMap<Id, Vec<f32>>,
    used_fonts: HashSet<FontInstanceKey>,
}

impl<Id> Default for TextFrame<Id> {
    fn default() -> Self {
        Self {
            prepared: HashMap::new(),
            prepared_sources: HashSet::new(),
            inline_fragments: HashMap::new(),
            inline_line_keys: HashMap::new(),
            used_fonts: HashSet::new(),
        }
    }
}

impl<Id> TextFrame<Id>
where
    Id: Copy + Eq + Hash,
{
    pub(crate) fn drain(&mut self, source: Id, commands: &mut Vec<PaintCmd>) -> bool {
        let prepared = self.prepared_sources.contains(&source);
        if let Some(mut source_commands) = self.prepared.remove(&source) {
            commands.append(&mut source_commands);
        }
        prepared
    }

    pub(crate) fn inline_fragments(&self, source: Id) -> Option<&[Fragment]> {
        self.inline_fragments.get(&source).map(Vec::as_slice)
    }

    fn record_inline_fragment(&mut self, source: Id, fragment: Fragment, line_y: f32) {
        let fragments = self.inline_fragments.entry(source).or_default();
        let line_keys = self.inline_line_keys.entry(source).or_default();
        if let Some(previous) = fragments.last_mut()
            && line_keys
                .last()
                .is_some_and(|previous_line| (previous_line - line_y).abs() <= 0.5)
            && fragment.x <= previous.x + previous.width + 0.5
        {
            let right = (previous.x + previous.width).max(fragment.x + fragment.width);
            let bottom = (previous.y + previous.height).max(fragment.y + fragment.height);
            previous.x = previous.x.min(fragment.x);
            previous.width = right - previous.x;
            previous.y = previous.y.min(fragment.y);
            previous.height = bottom - previous.y;
            return;
        }
        fragments.push(fragment);
        line_keys.push(line_y);
    }
}

struct SourceSpan<Id> {
    source: Option<Id>,
    owners: Vec<Id>,
    style: ComputedValues,
    range: Range<usize>,
}

struct InlineAtom<Id> {
    source: Id,
    owners: Vec<Id>,
    index: usize,
    fragment: Fragment,
    edge: bool,
}

enum ShapedItem<Id> {
    Text(ShapedRun<Id>),
    InlineBox {
        source: Id,
        owners: Vec<Id>,
        fragment: Fragment,
        edge: bool,
        line_y: f32,
    },
}

struct ShapedRun<Id> {
    source: Option<Id>,
    owners: Vec<Id>,
    fragment: Fragment,
    font_instance: FontInstanceKey,
    font_size: f32,
    color: ColorF,
    glyphs: Vec<GlyphInstance>,
}

fn translate_fragment(fragment: &mut Fragment, origin: (f32, f32)) {
    fragment.x += origin.0;
    fragment.y += origin.1;
}

fn is_inline<D>(dom: &D, styles: &StylePlane<D::NodeId>, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    match dom.kind(id) {
        NodeKind::Text => true,
        NodeKind::Element => styles
            .get(id)
            .is_some_and(|style| matches!(style.display, Display::Inline | Display::InlineBlock)),
        _ => false,
    }
}

struct InlineCollector<'a, D>
where
    D: LayoutDom,
{
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &'a FragmentPlane<D::NodeId>,
    already_prepared: &'a HashSet<D::NodeId>,
    owners: &'a mut Vec<D::NodeId>,
    text: &'a mut String,
    spans: &'a mut Vec<SourceSpan<D::NodeId>>,
    inline_boxes: &'a mut Vec<InlineAtom<D::NodeId>>,
    percentage_basis: f32,
}

impl<D> InlineCollector<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn collect(&mut self, id: D::NodeId, inherited: &ComputedValues) {
        match self.dom.kind(id) {
            NodeKind::Text => {
                if self.already_prepared.contains(&id) {
                    return;
                }
                let start = self.text.len();
                append_inline_text(self.text, self.dom.text(id).unwrap_or(""), inherited);
                if self.text.len() == start {
                    return;
                }
                self.spans.push(SourceSpan {
                    source: Some(id),
                    owners: self.owners.clone(),
                    style: inherited.clone(),
                    range: start..self.text.len(),
                });
            },
            NodeKind::Element => {
                let Some(style) = self.styles.get(id).cloned() else {
                    return;
                };
                if style.display == Display::None {
                    return;
                }
                if style.display == Display::InlineBlock {
                    if let Some(fragment) = self
                        .fragments
                        .atomic(id)
                        .or_else(|| self.fragments.get(id))
                        .copied()
                    {
                        self.inline_boxes.push(InlineAtom {
                            source: id,
                            owners: self.owners.clone(),
                            index: self.text.len(),
                            fragment,
                            edge: false,
                        });
                    }
                    return;
                }
                let ancestor_owners = self.owners.clone();
                self.push_edge(id, &style, &ancestor_owners, true);
                self.owners.push(id);
                for child in self.dom.dom_children(id) {
                    if is_inline(self.dom, self.styles, child) {
                        self.collect(child, &style);
                    }
                }
                self.owners.pop();
                self.push_edge(id, &style, &ancestor_owners, false);
            },
            _ => {},
        }
    }
}

impl<D> InlineCollector<'_, D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn push_edge(
        &mut self,
        source: D::NodeId,
        style: &ComputedValues,
        owners: &[D::NodeId],
        start: bool,
    ) {
        let edges = inline_decoration_edges(style, self.percentage_basis);
        let width = if start { edges.left } else { edges.right };
        if width <= 0.0 {
            return;
        }
        self.inline_boxes.push(InlineAtom {
            source,
            owners: owners.to_vec(),
            index: self.text.len(),
            fragment: Fragment {
                width,
                ..Fragment::default()
            },
            edge: true,
        });
    }
}

#[derive(Clone, Copy)]
struct InlineDecorationEdges {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

fn inline_decoration_edges(style: &ComputedValues, percentage_basis: f32) -> InlineDecorationEdges {
    let em = super::paint::used_font_size(style);
    InlineDecorationEdges {
        left: super::layout::length_percentage_px(style.padding_left.0, em, percentage_basis)
            + super::layout::border_width_px(style.border_left_style, style.border_left_width, em),
        right: super::layout::length_percentage_px(style.padding_right.0, em, percentage_basis)
            + super::layout::border_width_px(
                style.border_right_style,
                style.border_right_width,
                em,
            ),
        top: super::layout::length_percentage_px(style.padding_top.0, em, percentage_basis)
            + super::layout::border_width_px(style.border_top_style, style.border_top_width, em),
        bottom: super::layout::length_percentage_px(style.padding_bottom.0, em, percentage_basis)
            + super::layout::border_width_px(
                style.border_bottom_style,
                style.border_bottom_width,
                em,
            ),
    }
}

fn decorated_inline_fragment<Id>(
    styles: &StylePlane<Id>,
    source: Id,
    mut fragment: Fragment,
    percentage_basis: f32,
) -> Fragment
where
    Id: Copy + Eq + Hash,
{
    let Some(style) = styles.get(source) else {
        return fragment;
    };
    let edges = inline_decoration_edges(style, percentage_basis);
    fragment.y -= edges.top;
    fragment.height += edges.top + edges.bottom;
    fragment
}

fn normalized_text<'a>(source: &'a str, style: &ComputedValues) -> Cow<'a, str> {
    use livery::values::WhiteSpaceCollapse;

    if matches!(
        style.white_space_collapse,
        WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
    ) {
        return Cow::Borrowed(source);
    }
    Cow::Owned(source.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn append_inline_text(target: &mut String, source: &str, style: &ComputedValues) {
    use livery::values::WhiteSpaceCollapse;

    if matches!(
        style.white_space_collapse,
        WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
    ) {
        target.push_str(source);
        return;
    }

    let leading = source.chars().next().is_some_and(char::is_whitespace);
    let trailing = source.chars().next_back().is_some_and(char::is_whitespace);
    if leading && !target.is_empty() && !target.ends_with(char::is_whitespace) {
        target.push(' ');
    }
    for word in source.split_whitespace() {
        if !target.is_empty() && !target.ends_with(char::is_whitespace) {
            target.push(' ');
        }
        target.push_str(word);
    }
    if trailing && !target.is_empty() && !target.ends_with(char::is_whitespace) {
        target.push(' ');
    }
}

fn push_defaults(builder: &mut parley::RangedBuilder<'_, Brush>, style: &ComputedValues) {
    builder.push_default(StyleProperty::FontSize(super::paint::used_font_size(style)));
    builder.push_default(font_family(style));
    builder.push_default(StyleProperty::FontWeight(FontWeight::new(font_weight(
        style,
    ))));
    builder.push_default(StyleProperty::FontStyle(font_style(style)));
    builder.push_default(StyleProperty::Brush(brush(style, 0)));
    builder.push_default(line_height(style));
}

fn push_span(
    builder: &mut parley::RangedBuilder<'_, Brush>,
    style: &ComputedValues,
    range: Range<usize>,
    source_index: usize,
) {
    builder.push(
        StyleProperty::FontSize(super::paint::used_font_size(style)),
        range.clone(),
    );
    builder.push(font_family(style), range.clone());
    builder.push(
        StyleProperty::FontWeight(FontWeight::new(font_weight(style))),
        range.clone(),
    );
    builder.push(StyleProperty::FontStyle(font_style(style)), range.clone());
    builder.push(
        StyleProperty::Brush(brush(style, source_index)),
        range.clone(),
    );
    builder.push(line_height(style), range);
}

fn brush(style: &ComputedValues, source_index: usize) -> Brush {
    let color = resolve_color(style.color, ColorF::BLACK);
    Brush {
        color: [color.r, color.g, color.b, color.a],
        source_index,
    }
}

fn font_family(style: &ComputedValues) -> StyleProperty<'_, Brush> {
    let family = match &style.font_family {
        CssFontFamily::UserAgentDefault => FontFamily::from(GenericFamily::SansSerif),
        CssFontFamily::SystemUi => FontFamily::from(GenericFamily::SystemUi),
        CssFontFamily::Named(name) => FontFamily::Source(Cow::Borrowed(name)),
    };
    StyleProperty::FontFamily(family)
}

fn font_weight(style: &ComputedValues) -> f32 {
    match style.font_weight {
        CssFontWeight::Normal => 400.0,
        CssFontWeight::Bold | CssFontWeight::Bolder => 700.0,
        CssFontWeight::Lighter => 300.0,
        CssFontWeight::Number(value) => f32::from(value),
    }
}

fn font_style(style: &ComputedValues) -> FontStyle {
    match style.font_style {
        CssFontStyle::Normal => FontStyle::Normal,
        CssFontStyle::Italic | CssFontStyle::Oblique => FontStyle::Italic,
    }
}

fn line_height(style: &ComputedValues) -> StyleProperty<'static, Brush> {
    let value = match style.line_height {
        CssLineHeight::Normal => parley::LineHeight::MetricsRelative(1.0),
        CssLineHeight::Number(value) => parley::LineHeight::FontSizeRelative(value),
        CssLineHeight::Value(_) => parley::LineHeight::Absolute(super::layout::line_height_px(
            &style.line_height,
            super::paint::used_font_size(style),
        )),
    };
    StyleProperty::LineHeight(value)
}

fn content_key(bytes: &[u8], index: u32) -> FontInstanceKey {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes.iter().copied().chain(index.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    FontInstanceKey::new(IdNamespace((hash >> 32) as u32), hash as u32)
}
