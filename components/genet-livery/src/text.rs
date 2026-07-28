//! Clean-room Parley adapter for Livery inline formatting.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    hash::Hash,
    ops::Range,
    sync::Arc,
};

use buckram::{
    BoxId, BoxOrigin, CssBoxTree, DisplayInside, DisplayOutside, FloatLineConstraints,
    FormattingContextKind,
};
use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        Display, FontFamily as CssFontFamily, FontStyle as CssFontStyle,
        FontWeight as CssFontWeight, LineHeight as CssLineHeight, Margin, Position, Spacing,
        TextAlign, TextWrapMode, VerticalAlign,
    },
};
use paint_list_api::{
    ColorF, CommonPlacement, FontInstanceKey, FontResource, GlyphInstance, IdNamespace,
    LayoutPoint, PaintCmd, TextOptions, TextRunItem,
};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, GenericFamily,
    InlineBox, InlineBoxKind, LayoutContext, PositionedLayoutItem, StyleProperty,
    layout::YieldData,
};

use crate::{LiveryLayout, StylePlane, layout::Fragment, paint::resolve_color};

pub(crate) trait FragmentLookup<Id> {
    fn rect(&self, id: Id) -> Option<&Fragment>;

    fn atomic_rect(&self, id: Id) -> Option<&Fragment> {
        self.rect(id)
    }

    fn atomic_box_rect(&self, _box_id: BoxId) -> Option<&Fragment> {
        None
    }
}

impl<Id> FragmentLookup<Id> for LiveryLayout<Id>
where
    Id: Copy + Eq + Hash,
{
    fn rect(&self, id: Id) -> Option<&Fragment> {
        self.get(id).map(|fragment| &**fragment)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Brush {
    color: [f32; 4],
    source_index: usize,
}

/// One retained text rectangle in viewport coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A source-node text range in rendered byte space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRange<Id> {
    pub anchor_node: Id,
    pub anchor_offset: usize,
    pub focus_node: Id,
    pub focus_offset: usize,
}

/// A non-collapsed retained selection over Livery's shaped text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSelection<Id> {
    pub range: TextRange<Id>,
    pub source_nodes: Vec<Id>,
    pub rects: Vec<TextRect>,
    pub text: String,
}

fn break_inline_lines(
    layout: &mut parley::Layout<Brush>,
    width: f32,
    wraps: bool,
    constraints: Option<&FloatLineConstraints>,
) {
    if !width.is_finite() || width <= 0.0 {
        layout.break_all_lines(None);
        return;
    }
    let Some(constraints) = constraints else {
        layout.break_all_lines(wraps.then_some(width));
        return;
    };
    let layout_max_advance = if wraps { width } else { f32::INFINITY };

    {
        let mut breaker = layout.break_lines();
        breaker
            .state_mut()
            .set_layout_max_advance(layout_max_advance);
        'lines: loop {
            let mut line_top = breaker.committed_y() as f32;
            let mut available = constraints.horizontal_physical_space(line_top, 0.0);
            for _ in 0..32 {
                {
                    let state = breaker.state_mut();
                    state.set_line_x(available.inline_start);
                    state.set_line_y(f64::from(line_top));
                    state.set_line_max_advance(if wraps {
                        available.inline_size
                    } else {
                        f32::INFINITY
                    });
                }
                let Some(yielded) = breaker.break_next() else {
                    break 'lines;
                };
                let YieldData::LineBreak(line) = yielded else {
                    debug_assert!(false, "in-flow inline layout yielded a non-line break");
                    break 'lines;
                };

                let spanning =
                    constraints.horizontal_physical_space(line_top, line.line_height.max(0.0));
                if ((spanning.inline_start - available.inline_start).abs() > 0.01
                    || (spanning.inline_size - available.inline_size).abs() > 0.01)
                    && breaker.revert()
                {
                    available = spanning;
                    continue;
                }

                if line.advance > available.inline_size + 0.01
                    && let Some(next_top) = constraints.next_wider_block_start(
                        line_top,
                        line.line_height.max(0.0),
                        available.inline_size,
                    )
                    && breaker.revert()
                {
                    line_top = next_top;
                    available =
                        constraints.horizontal_physical_space(line_top, line.line_height.max(0.0));
                    continue;
                }
                break;
            }
        }
    }
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

    pub(crate) fn register_font_bytes(&mut self, bytes: Vec<u8>) {
        self.font_context
            .collection
            .register_fonts(parley::fontique::Blob::new(Arc::new(bytes)), None);
    }

    /// Format one consecutive inline group for both layout and paint.
    pub(crate) fn format_inline_group<D, F>(
        &mut self,
        dom: &D,
        styles: &StylePlane<D::NodeId>,
        boxes: &CssBoxTree<D::NodeId>,
        fragments: &F,
        request: InlineRequest<'_>,
    ) -> Option<InlineLayout<BoxId>>
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
        F: FragmentLookup<D::NodeId>,
    {
        let parent_style = request.parent_style;
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut inline_boxes = Vec::new();
        let mut owners = Vec::new();
        {
            let mut collector = BoxInlineCollector {
                dom,
                styles,
                boxes,
                fragments,
                owners: &mut owners,
                text: &mut text,
                spans: &mut spans,
                inline_boxes: &mut inline_boxes,
                negative_margin_offset: 0.0,
                percentage_basis: request.width,
            };
            for root in request.roots {
                collector.collect(*root, parent_style);
            }
        }
        if spans.is_empty() && inline_boxes.is_empty() {
            return None;
        }

        let items = self.shape(
            &text,
            &mut spans,
            &inline_boxes,
            request.width,
            parent_style,
            request.line_constraints,
        );
        let text_sources = spans
            .iter()
            .filter_map(|span| {
                span.source.map(|source| TextSource {
                    source,
                    range: span.range.clone(),
                })
            })
            .collect();
        let zero_line_strut = text.is_empty()
            && spans.is_empty()
            && !inline_boxes.is_empty()
            && super::layout::line_height_px(
                &parent_style.line_height,
                super::paint::used_font_size(parent_style),
            ) <= 0.0;
        let zero_line_minimal_alignment = zero_line_strut
            && inline_boxes.iter().all(|inline_box| {
                matches!(
                    inline_box.vertical_align,
                    VerticalAlign::Top
                        | VerticalAlign::TextTop
                        | VerticalAlign::Bottom
                        | VerticalAlign::TextBottom
                )
            });
        let strut_center_height = if zero_line_strut {
            let mut strut_spans = Vec::<SourceSpan<()>>::new();
            let strut_items = self.shape::<()>(
                "\u{200b}",
                &mut strut_spans,
                &[],
                request.width,
                parent_style,
                None,
            );
            strut_items.into_iter().find_map(|item| match item {
                ShapedItem::Text(run) => Some(
                    (run.line_baseline - (run.line_block_min + run.line_block_max) * 0.5).abs(),
                ),
                ShapedItem::InlineBox { .. } => None,
            })
        } else {
            None
        };
        let mut right = 0.0_f32;
        let mut top = f32::INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        let empty_line_height = inline_boxes
            .iter()
            .filter(|inline_box| {
                !inline_box.edge && !inline_box.paint && inline_box.line_width == 0.0
            })
            .map(|inline_box| inline_box.line_box_height)
            .reduce(f32::max);
        let parent_line_height = super::layout::line_height_px(
            &parent_style.line_height,
            super::paint::used_font_size(parent_style),
        );
        for item in &items {
            let fragment = match item {
                ShapedItem::Text(run) => run.line_fragment,
                ShapedItem::InlineBox { line_fragment, .. } => *line_fragment,
            };
            right = right.max(fragment.x + fragment.width);
            top = top.min(fragment.y);
            bottom = bottom.max(fragment.y + fragment.height);
        }
        if top.is_finite() && bottom.is_finite() {
            let measured_height = (bottom - top).max(strut_center_height.unwrap_or(0.0));
            Some(InlineLayout {
                width: right.max(0.0),
                height: if zero_line_minimal_alignment {
                    0.0
                } else if let Some(empty_line_height) = empty_line_height {
                    empty_line_height.max(parent_line_height)
                } else {
                    measured_height
                },
                items,
                text,
                text_sources,
            })
        } else {
            Some(InlineLayout {
                width: 0.0,
                height: 0.0,
                items,
                text,
                text_sources,
            })
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
        fragments: &LiveryLayout<D::NodeId>,
        parent: D::NodeId,
        parent_style: &ComputedValues,
    ) where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        let Some(parent_box) = fragments.boxes().principal_box(parent) else {
            return;
        };
        if fragments.boxes()[parent_box].formatting_context != Some(FormattingContextKind::Inline) {
            return;
        }
        let Some(parent_fragment) = frame
            .inline_fragments(parent)
            .and_then(|fragments| fragments.first())
            .or_else(|| fragments.get(parent).map(|fragment| &**fragment))
            .copied()
        else {
            return;
        };
        let mut inline_parent_style = parent_style.clone();
        if matches!(parent_style.position, Position::Absolute | Position::Fixed) {
            inline_parent_style.vertical_align = VerticalAlign::Baseline;
        }
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
                    (&parent_fragment, &inline_parent_style),
                    parent,
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
            (&parent_fragment, &inline_parent_style),
            parent,
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
            negative_margin_offset: 0.0,
        }];
        for item in self.shape(text.as_ref(), &mut spans, &[], fragment.width, style, None) {
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

    #[allow(clippy::too_many_arguments)]
    fn flush_group<D>(
        &mut self,
        frame: &mut TextFrame<D::NodeId>,
        dom: &D,
        styles: &StylePlane<D::NodeId>,
        fragments: &LiveryLayout<D::NodeId>,
        roots: &[D::NodeId],
        parent: (&Fragment, &ComputedValues),
        owner: D::NodeId,
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
        let mut owners = vec![owner];
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
                negative_margin_offset: 0.0,
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
        let mut visual_commands = Vec::new();
        let mut prepared_sources = Vec::new();
        let text_sources = spans
            .iter()
            .filter_map(|span| Some((span.source?, text.get(span.range.clone())?.to_owned())))
            .collect();
        frame.record_text_group(text_sources);
        for item in self.shape(
            &text,
            &mut spans,
            &inline_boxes,
            parent_fragment.width,
            parent_style,
            None,
        ) {
            match item {
                ShapedItem::Text(mut run) => {
                    let Some(source) = run.source else {
                        continue;
                    };
                    translate_fragment(&mut run.fragment, origin);
                    let line_y = run.line_y + origin.1;
                    for glyph in &mut run.glyphs {
                        glyph.point.x += origin.0;
                        glyph.point.y += origin.1;
                    }
                    frame.record_inline_fragment(source, run.fragment, line_y);
                    for cluster in &run.clusters {
                        let mut cluster_fragment = cluster.fragment;
                        translate_fragment(&mut cluster_fragment, origin);
                        frame.record_text_cluster(
                            cluster.source,
                            cluster.range.clone(),
                            cluster_fragment,
                            cluster.rtl,
                        );
                    }
                    for owner in &run.owners {
                        frame.record_inline_fragment(
                            *owner,
                            decorated_inline_fragment(
                                styles,
                                *owner,
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
                    if frame.prepared_sources.insert(source) {
                        prepared_sources.push(source);
                    }
                    visual_commands.push(PreparedCommand {
                        owners: run.owners,
                        command,
                    });
                },
                ShapedItem::InlineBox {
                    source,
                    owners,
                    mut fragment,
                    line_fragment: _,
                    edge,
                    paint,
                    mut line_y,
                } => {
                    translate_fragment(&mut fragment, origin);
                    line_y += origin.1;
                    if paint {
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
                    }
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
        frame.record_prepared_group(prepared_sources, visual_commands);
    }

    fn shape<Id>(
        &mut self,
        text: &str,
        spans: &mut [SourceSpan<Id>],
        inline_boxes: &[InlineAtom<Id>],
        width: f32,
        root_style: &ComputedValues,
        line_constraints: Option<&FloatLineConstraints>,
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
                width: inline_box.line_width,
                height: if inline_box.edge {
                    0.0
                } else {
                    inline_box.line_box_height
                },
            });
        }
        let mut layout = builder.build(text);
        break_inline_lines(
            &mut layout,
            width,
            root_style.text_wrap_mode == TextWrapMode::Wrap,
            line_constraints,
        );
        layout.align(
            text_alignment(root_style.text_align),
            AlignmentOptions::default(),
        );

        let mut result = Vec::new();
        for line in layout.lines() {
            let source_metrics = *line.metrics();
            let content_height =
                (source_metrics.block_max_coord - source_metrics.block_min_coord).max(0.0);
            let (has_text_top, has_text_bottom) =
                line.items()
                    .fold((false, false), |(has_text_top, has_text_bottom), item| {
                        let PositionedLayoutItem::GlyphRun(run) = item else {
                            return (has_text_top, has_text_bottom);
                        };
                        let value = spans
                            .get(run.style().brush.source_index)
                            .map(|span| span.style.vertical_align);
                        (
                            has_text_top || matches!(value, Some(VerticalAlign::TextTop)),
                            has_text_bottom || matches!(value, Some(VerticalAlign::TextBottom)),
                        )
                    });
            let requested_line_height = std::iter::once(root_style)
                .chain(spans.iter().map(|span| &span.style))
                .filter_map(explicit_line_height)
                .chain(
                    inline_boxes
                        .iter()
                        .map(|inline_box| inline_box.line_box_height)
                        .filter(|height| *height > 0.0),
                )
                .reduce(f32::max);
            let has_in_flow_atom = inline_boxes.iter().any(|inline_box| !inline_box.edge);
            let line_box_height = if has_in_flow_atom {
                source_metrics
                    .line_height
                    .max(content_height)
                    .max(requested_line_height.unwrap_or(0.0))
            } else {
                requested_line_height.unwrap_or(source_metrics.line_height.max(content_height))
            } + if has_text_top || has_text_bottom {
                (source_metrics.leading * 0.5).max(0.0)
            } else {
                0.0
            }
            .max(0.0);
            let extra_leading = (line_box_height - content_height).max(0.0);
            let edge_leading = if has_text_top || has_text_bottom {
                (source_metrics.leading * 0.5).max(0.0)
            } else {
                0.0
            };
            let common_vertical_shift = if has_text_bottom {
                edge_leading - extra_leading * 0.5
            } else if has_text_top {
                -extra_leading * 0.5
            } else {
                0.0
            };
            let mut metrics = source_metrics;
            metrics.line_height = line_box_height;
            metrics.block_max_coord = metrics.block_min_coord + metrics.line_height;
            metrics.baseline += extra_leading * 0.5;
            let strut_height = super::layout::line_height_px(
                &root_style.line_height,
                super::paint::used_font_size(root_style),
            );
            let empty_line_shift = inline_boxes
                .iter()
                .filter(|inline_box| {
                    !inline_box.edge && !inline_box.paint && inline_box.line_width == 0.0
                })
                .map(|inline_box| {
                    ((inline_box.line_box_height - strut_height).max(0.0)) * 0.5
                        + ((metrics.block_max_coord
                            - metrics.block_min_coord
                            - inline_box.line_box_height)
                            .max(0.0)
                            * 0.5)
                })
                .fold(0.0, f32::max);
            for item in line.items() {
                match item {
                    PositionedLayoutItem::GlyphRun(run) => {
                        let parley_run = run.run();
                        let brush = &run.style().brush;
                        let span = spans.get(brush.source_index);
                        let negative_margin_offset =
                            span.map_or(0.0, |span| span.negative_margin_offset);
                        let vertical_shift = span.map_or(0.0, |span| {
                            let edge_shift = match span.style.vertical_align {
                                VerticalAlign::TextTop if has_text_top => edge_leading,
                                VerticalAlign::TextBottom if has_text_bottom => -edge_leading,
                                _ => 0.0,
                            };
                            common_vertical_shift
                                + edge_shift
                                + vertical_align_shift(
                                    span.style.vertical_align,
                                    super::paint::used_font_size(&span.style),
                                    super::layout::line_height_px(
                                        &span.style.line_height,
                                        super::paint::used_font_size(&span.style),
                                    ),
                                    &metrics,
                                    source_metrics.block_min_coord,
                                    super::layout::line_height_px(
                                        &span.style.line_height,
                                        super::paint::used_font_size(&span.style),
                                    ),
                                    false,
                                )
                        });
                        let mut glyphs = run
                            .positioned_glyphs()
                            .map(|glyph| GlyphInstance {
                                index: glyph.id,
                                point: LayoutPoint::new(glyph.x + negative_margin_offset, glyph.y),
                            })
                            .collect::<Vec<_>>();
                        if glyphs.is_empty() {
                            continue;
                        }
                        for glyph in &mut glyphs {
                            glyph.point.y +=
                                vertical_shift + extra_leading * 0.5 - empty_line_shift;
                        }
                        let line_fragment_y = metrics.block_min_coord
                            + if has_text_top || has_text_bottom {
                                common_vertical_shift
                            } else {
                                vertical_shift
                            };
                        let mut cluster_x = run.offset() + negative_margin_offset;
                        let mut clusters = Vec::new();
                        for cluster in parley_run.visual_clusters() {
                            let advance = cluster.advance().max(0.0);
                            let global = cluster.text_range();
                            let source_span = spans
                                .get(cluster.first_style().brush.source_index)
                                .and_then(|span| span.source.map(|source| (source, span)));
                            if let Some((source, source_span)) = source_span {
                                let start = global.start.max(source_span.range.start);
                                let end = global.end.min(source_span.range.end);
                                if start < end {
                                    clusters.push(ShapedCluster {
                                        source,
                                        range: (start - source_span.range.start)
                                            ..(end - source_span.range.start),
                                        fragment: Fragment {
                                            x: cluster_x,
                                            y: line_fragment_y,
                                            width: advance,
                                            height: metrics.line_height.max(0.0),
                                        },
                                        rtl: cluster.is_rtl(),
                                    });
                                }
                            }
                            cluster_x += advance;
                        }
                        let [red, green, blue, alpha] = brush.color;
                        let paint_height = metrics.line_height.max(content_height);
                        result.push(ShapedItem::Text(ShapedRun {
                            source: span.and_then(|span| span.source),
                            owners: span.map_or_else(Vec::new, |span| span.owners.clone()),
                            // Keep the font-content metrics separate from the
                            // explicit line box.  Zero-height struts use this
                            // center to place replaced atoms without turning
                            // glyph overflow into flow height.
                            line_baseline: source_metrics.baseline,
                            line_block_min: source_metrics.block_min_coord,
                            line_block_max: source_metrics.block_max_coord,
                            line_y: metrics.block_min_coord,
                            fragment: Fragment {
                                x: run.offset() + negative_margin_offset,
                                y: metrics.block_min_coord + vertical_shift,
                                width: run.advance().max(0.0),
                                height: paint_height.max(0.0),
                            },
                            line_fragment: Fragment {
                                x: run.offset() + negative_margin_offset,
                                y: line_fragment_y,
                                width: run.advance().max(0.0),
                                height: metrics.line_height.max(0.0),
                            },
                            font_instance: self.intern_font(parley_run.font()),
                            font_size: parley_run.font_size(),
                            color: ColorF::new(red, green, blue, alpha),
                            glyphs,
                            clusters,
                        }));
                    },
                    PositionedLayoutItem::InlineBox(positioned) => {
                        let Some(inline_box) = usize::try_from(positioned.id)
                            .ok()
                            .and_then(|index| inline_boxes.get(index))
                        else {
                            continue;
                        };
                        let line_height = (metrics.block_max_coord - metrics.block_min_coord)
                            .max(positioned.height)
                            .max(0.0);
                        let height = if inline_box.edge {
                            line_height
                        } else {
                            positioned.height
                        };
                        let base_y = if inline_box.edge {
                            metrics.block_min_coord
                        } else {
                            positioned.y
                        };
                        let vertical_shift = if inline_box.edge {
                            0.0
                        } else {
                            vertical_align_shift(
                                inline_box.vertical_align,
                                inline_box.font_size,
                                inline_box.line_height,
                                &metrics,
                                base_y,
                                height,
                                true,
                            )
                        };
                        result.push(ShapedItem::InlineBox {
                            source: inline_box.source,
                            owners: inline_box.owners.clone(),
                            fragment: Fragment {
                                x: positioned.x
                                    + if inline_box.edge {
                                        0.0
                                    } else {
                                        inline_box.margin_left
                                    },
                                y: base_y
                                    + vertical_shift
                                    + if inline_box.edge {
                                        0.0
                                    } else {
                                        inline_box.margin_top
                                    },
                                width: if inline_box.edge {
                                    positioned.width
                                } else {
                                    inline_box.fragment.width
                                },
                                height: if inline_box.edge {
                                    height
                                } else {
                                    inline_box.fragment.height
                                },
                            },
                            line_fragment: Fragment {
                                x: positioned.x,
                                y: base_y
                                    + if inline_box.edge && (has_text_top || has_text_bottom) {
                                        common_vertical_shift
                                    } else {
                                        vertical_shift
                                    },
                                width: positioned.width,
                                height: line_height,
                            },
                            edge: inline_box.edge,
                            paint: inline_box.paint,
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

pub(crate) struct InlineRequest<'a> {
    pub(crate) roots: &'a [BoxId],
    pub(crate) parent_style: &'a ComputedValues,
    pub(crate) width: f32,
    pub(crate) line_constraints: Option<&'a FloatLineConstraints>,
}

pub(crate) struct InlineLayout<Source> {
    width: f32,
    height: f32,
    items: Vec<ShapedItem<Source>>,
    text: String,
    text_sources: Vec<TextSource<Source>>,
}

pub(crate) struct InlinePlacement<Source> {
    pub(crate) fragments: HashMap<Source, Vec<Fragment>>,
    line_keys: HashMap<Source, Vec<f32>>,
}

impl<Source> Default for InlinePlacement<Source> {
    fn default() -> Self {
        Self {
            fragments: HashMap::new(),
            line_keys: HashMap::new(),
        }
    }
}

impl<Source> InlinePlacement<Source>
where
    Source: Copy + Eq + Hash,
{
    fn record(&mut self, source: Source, fragment: Fragment, line_y: f32) {
        let fragments = self.fragments.entry(source).or_default();
        let line_keys = self.line_keys.entry(source).or_default();
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

impl<Source> InlineLayout<Source>
where
    Source: Copy + Eq + Hash,
{
    pub(crate) fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }

    /// Return the first and last text-line baselines relative to this inline
    /// formatting context's block-start edge. Atomic-only lines deliberately
    /// leave this unset so their formatting context can synthesize the
    /// block-end fallback instead.
    pub(crate) fn baselines(&self) -> Option<(f32, f32)> {
        let mut first = None::<f32>;
        let mut last = None::<f32>;
        for item in &self.items {
            let ShapedItem::Text(run) = item else {
                continue;
            };
            // Parley reports this metric in the inline layout's coordinate
            // space already. `line_y` locates the painted fragment, not an
            // extra offset to add to the baseline output.
            let baseline = run.line_baseline;
            if !baseline.is_finite() || baseline < 0.0 {
                continue;
            }
            first = Some(first.map_or(baseline, |current| current.min(baseline)));
            last = Some(last.map_or(baseline, |current| current.max(baseline)));
        }
        first.zip(last)
    }

    pub(crate) fn place<Id, Resolve>(
        &self,
        frame: &mut TextFrame<Id>,
        styles: &StylePlane<Id>,
        mut node_for: Resolve,
        origin: (f32, f32),
        percentage_basis: f32,
    ) -> InlinePlacement<Source>
    where
        Id: Copy + Eq + Hash,
        Resolve: FnMut(Source) -> Option<Id>,
    {
        let container = Fragment {
            x: origin.0,
            y: origin.1,
            width: percentage_basis,
            height: self.height,
        };
        let mut visual_commands = Vec::new();
        let mut prepared_sources = Vec::new();
        let mut placement = InlinePlacement::default();
        let text_sources = self
            .text_sources
            .iter()
            .filter_map(|source| {
                Some((
                    node_for(source.source)?,
                    self.text.get(source.range.clone())?.to_owned(),
                ))
            })
            .collect();
        frame.record_text_group(text_sources);

        for item in &self.items {
            match item {
                ShapedItem::Text(run) => {
                    let Some(source) = run.source else {
                        continue;
                    };
                    let mut fragment = run.fragment;
                    translate_fragment(&mut fragment, origin);
                    let line_y = run.line_y + origin.1;
                    let mut glyphs = run.glyphs.clone();
                    for glyph in &mut glyphs {
                        glyph.point.x += origin.0;
                        glyph.point.y += origin.1;
                    }
                    placement.record(source, fragment, line_y);
                    let Some(source_node) = node_for(source) else {
                        continue;
                    };
                    frame.record_inline_fragment(source_node, fragment, line_y);
                    for cluster in &run.clusters {
                        let Some(cluster_node) = node_for(cluster.source) else {
                            continue;
                        };
                        let mut cluster_fragment = cluster.fragment;
                        translate_fragment(&mut cluster_fragment, origin);
                        frame.record_text_cluster(
                            cluster_node,
                            cluster.range.clone(),
                            cluster_fragment,
                            cluster.rtl,
                        );
                    }
                    let mut command_owners = Vec::new();
                    for owner in &run.owners {
                        let Some(owner_node) = node_for(*owner) else {
                            continue;
                        };
                        let decorated = decorated_inline_fragment(
                            styles,
                            owner_node,
                            fragment,
                            percentage_basis,
                        );
                        placement.record(*owner, decorated, line_y);
                        frame.record_inline_fragment(owner_node, decorated, line_y);
                        command_owners.push(owner_node);
                    }
                    frame.used_fonts.insert(run.font_instance);
                    if frame.prepared_sources.insert(source_node) {
                        prepared_sources.push(source_node);
                    }
                    visual_commands.push(PreparedCommand {
                        owners: command_owners,
                        command: PaintCmd::DrawText(TextRunItem {
                            placement: CommonPlacement::new(super::paint::bounds(&container)),
                            font_instance: run.font_instance,
                            font_size: run.font_size,
                            color: run.color,
                            glyphs,
                            options: TextOptions::default(),
                        }),
                    });
                },
                ShapedItem::InlineBox {
                    source,
                    owners,
                    fragment,
                    edge,
                    paint,
                    line_y,
                    ..
                } => {
                    let mut fragment = *fragment;
                    translate_fragment(&mut fragment, origin);
                    let line_y = *line_y + origin.1;
                    let source_node = node_for(*source);
                    if *paint {
                        let source_fragment = if *edge {
                            source_node.map_or(fragment, |node| {
                                decorated_inline_fragment(styles, node, fragment, percentage_basis)
                            })
                        } else {
                            fragment
                        };
                        placement.record(*source, source_fragment, line_y);
                        if let Some(node) = source_node {
                            frame.record_inline_fragment(node, source_fragment, line_y);
                        }
                    }
                    for owner in owners {
                        let Some(owner_node) = node_for(*owner) else {
                            continue;
                        };
                        let decorated = decorated_inline_fragment(
                            styles,
                            owner_node,
                            fragment,
                            percentage_basis,
                        );
                        placement.record(*owner, decorated, line_y);
                        frame.record_inline_fragment(owner_node, decorated, line_y);
                    }
                },
            }
        }
        frame.record_prepared_group(prepared_sources, visual_commands);
        placement
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TextFrame<Id> {
    prepared_groups: Vec<Vec<PreparedCommand<Id>>>,
    source_groups: HashMap<Id, usize>,
    prepared_sources: HashSet<Id>,
    inline_fragments: HashMap<Id, Vec<Fragment>>,
    inline_line_keys: HashMap<Id, Vec<f32>>,
    painted_decorations: HashSet<Id>,
    used_fonts: HashSet<FontInstanceKey>,
    text_order: Vec<Id>,
    text_values: HashMap<Id, String>,
    text_groups: HashMap<Id, usize>,
    text_clusters: Vec<RetainedTextCluster<Id>>,
    next_text_group: usize,
}

impl<Id> Default for TextFrame<Id> {
    fn default() -> Self {
        Self {
            prepared_groups: Vec::new(),
            source_groups: HashMap::new(),
            prepared_sources: HashSet::new(),
            inline_fragments: HashMap::new(),
            inline_line_keys: HashMap::new(),
            painted_decorations: HashSet::new(),
            used_fonts: HashSet::new(),
            text_order: Vec::new(),
            text_values: HashMap::new(),
            text_groups: HashMap::new(),
            text_clusters: Vec::new(),
            next_text_group: 0,
        }
    }
}

impl<Id> TextFrame<Id>
where
    Id: Copy + Eq + Hash,
{
    pub(crate) fn drain(
        &mut self,
        source: Id,
        inline_owner: Option<Id>,
        excluded_roots: Option<&HashSet<Id>>,
        commands: &mut Vec<PaintCmd>,
    ) -> bool {
        let prepared = self.prepared_sources.contains(&source);
        if let Some(group) = self.source_groups.get(&source).copied() {
            let mut retained = Vec::new();
            for prepared in std::mem::take(&mut self.prepared_groups[group]) {
                let belongs_to_owner =
                    inline_owner.is_none_or(|owner| prepared.owners.contains(&owner));
                let belongs_to_child_context = excluded_roots
                    .is_some_and(|roots| prepared.owners.iter().any(|owner| roots.contains(owner)));
                if belongs_to_owner && !belongs_to_child_context {
                    commands.push(prepared.command);
                } else {
                    retained.push(prepared);
                }
            }
            self.prepared_groups[group] = retained;
        }
        prepared
    }

    fn record_prepared_group(&mut self, sources: Vec<Id>, commands: Vec<PreparedCommand<Id>>) {
        if sources.is_empty() {
            return;
        }
        let group = self.prepared_groups.len();
        self.prepared_groups.push(commands);
        for source in sources {
            self.source_groups.insert(source, group);
        }
    }

    pub(crate) fn mark_decoration_painted(&mut self, source: Id) -> bool {
        self.painted_decorations.insert(source)
    }

    pub(crate) fn inline_fragments(&self, source: Id) -> Option<&[Fragment]> {
        self.inline_fragments.get(&source).map(Vec::as_slice)
    }

    pub(crate) fn first_inline_line(&self, source: Id) -> Option<f32> {
        self.inline_line_keys
            .get(&source)
            .and_then(|lines| lines.first().copied())
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

    fn record_text_group(&mut self, sources: Vec<(Id, String)>) {
        if sources.is_empty() {
            return;
        }
        let group = self.next_text_group;
        self.next_text_group = self.next_text_group.saturating_add(1);
        for (source, text) in sources {
            if self.text_values.contains_key(&source) {
                continue;
            }
            self.text_order.push(source);
            self.text_values.insert(source, text);
            self.text_groups.insert(source, group);
        }
    }

    fn record_text_cluster(
        &mut self,
        source: Id,
        range: Range<usize>,
        fragment: Fragment,
        rtl: bool,
    ) {
        self.text_clusters.push(RetainedTextCluster {
            source,
            range,
            fragment,
            rtl,
        });
    }

    pub(crate) fn text_position_at_point<F>(
        &self,
        x: f32,
        y: f32,
        mut transform: F,
    ) -> Option<(Id, usize)>
    where
        F: FnMut(Id, Fragment) -> TextRect,
    {
        let mut closest = None::<(f32, &RetainedTextCluster<Id>, TextRect)>;
        for cluster in &self.text_clusters {
            let rect = transform(cluster.source, cluster.fragment);
            if rect.height <= 0.0 || y < rect.y || y >= rect.y + rect.height {
                continue;
            }
            let distance = if x < rect.x {
                rect.x - x
            } else if x > rect.x + rect.width {
                x - (rect.x + rect.width)
            } else {
                0.0
            };
            if closest.as_ref().is_none_or(|(best, _, _)| distance < *best) {
                closest = Some((distance, cluster, rect));
            }
        }
        let (_, cluster, rect) = closest?;
        let leading = x <= rect.x + rect.width * 0.5;
        let offset = match (cluster.rtl, leading) {
            (false, true) | (true, false) => cluster.range.start,
            (false, false) | (true, true) => cluster.range.end,
        };
        Some((cluster.source, offset))
    }

    pub(crate) fn find_text_range(&self, needle: &str) -> Option<TextRange<Id>> {
        let needle = needle.to_ascii_lowercase();
        if needle.is_empty() {
            return None;
        }
        self.text_order.iter().find_map(|source| {
            let text = self.text_values.get(source)?;
            let start = text.to_ascii_lowercase().find(&needle)?;
            Some(TextRange {
                anchor_node: *source,
                anchor_offset: start,
                focus_node: *source,
                focus_offset: start + needle.len(),
            })
        })
    }

    pub(crate) fn caret_rect<F>(
        &self,
        source: Id,
        offset: usize,
        mut transform: F,
    ) -> Option<TextRect>
    where
        F: FnMut(Id, Fragment) -> TextRect,
    {
        let cluster = self
            .text_clusters
            .iter()
            .filter(|cluster| cluster.source == source)
            .min_by_key(|cluster| {
                if offset < cluster.range.start {
                    cluster.range.start - offset
                } else if offset > cluster.range.end {
                    offset - cluster.range.end
                } else {
                    0
                }
            })?;
        let rect = transform(source, cluster.fragment);
        let length = cluster.range.end.saturating_sub(cluster.range.start);
        let fraction = if length == 0 {
            0.0
        } else {
            (offset.clamp(cluster.range.start, cluster.range.end) - cluster.range.start) as f32
                / length as f32
        };
        let fraction = if cluster.rtl {
            1.0 - fraction
        } else {
            fraction
        };
        Some(TextRect {
            x: rect.x + rect.width * fraction,
            y: rect.y,
            width: 1.0,
            height: rect.height,
        })
    }

    pub(crate) fn text_selection<F>(
        &self,
        range: TextRange<Id>,
        mut transform: F,
    ) -> Option<TextSelection<Id>>
    where
        F: FnMut(Id, Fragment) -> TextRect,
    {
        let anchor_index = self
            .text_order
            .iter()
            .position(|source| *source == range.anchor_node)?;
        let focus_index = self
            .text_order
            .iter()
            .position(|source| *source == range.focus_node)?;
        let ((start_index, start_offset), (end_index, end_offset)) = if anchor_index < focus_index
            || (anchor_index == focus_index && range.anchor_offset <= range.focus_offset)
        {
            (
                (anchor_index, range.anchor_offset),
                (focus_index, range.focus_offset),
            )
        } else {
            (
                (focus_index, range.focus_offset),
                (anchor_index, range.anchor_offset),
            )
        };
        if start_index == end_index && start_offset == end_offset {
            return None;
        }

        let ordered = TextRange {
            anchor_node: self.text_order[start_index],
            anchor_offset: start_offset,
            focus_node: self.text_order[end_index],
            focus_offset: end_offset,
        };
        let mut selected = Vec::new();
        let mut text = String::new();
        let mut previous_group = None;
        for index in start_index..=end_index {
            let source = self.text_order[index];
            let value = self.text_values.get(&source)?;
            let start = if index == start_index {
                start_offset.min(value.len())
            } else {
                0
            };
            let end = if index == end_index {
                end_offset.min(value.len())
            } else {
                value.len()
            };
            if start >= end {
                continue;
            }
            if let Some(slice) = value.get(start..end) {
                let group = self.text_groups.get(&source).copied();
                if !text.is_empty() && previous_group != group {
                    text.push('\n');
                }
                text.push_str(slice);
                previous_group = group;
                selected.push((source, start, end));
            }
        }
        if text.is_empty() {
            return None;
        }

        let mut rects = Vec::new();
        for cluster in &self.text_clusters {
            let Some((_, start, end)) = selected
                .iter()
                .find(|(source, _, _)| *source == cluster.source)
            else {
                continue;
            };
            let start = (*start).max(cluster.range.start);
            let end = (*end).min(cluster.range.end);
            if start >= end {
                continue;
            }
            let mut rect = transform(cluster.source, cluster.fragment);
            let length = cluster.range.end.saturating_sub(cluster.range.start);
            if length > 0 {
                let start_fraction = (start - cluster.range.start) as f32 / length as f32;
                let end_fraction = (end - cluster.range.start) as f32 / length as f32;
                let (left, right) = if cluster.rtl {
                    (1.0 - end_fraction, 1.0 - start_fraction)
                } else {
                    (start_fraction, end_fraction)
                };
                rect.x += rect.width * left;
                rect.width *= right - left;
            }
            if rect.width > 0.0 && rect.height > 0.0 {
                rects.push(rect);
            }
        }
        rects.sort_by(|left, right| {
            left.y
                .total_cmp(&right.y)
                .then_with(|| left.x.total_cmp(&right.x))
        });
        let mut merged = Vec::<TextRect>::new();
        for rect in rects {
            if let Some(previous) = merged.last_mut()
                && (previous.y - rect.y).abs() <= 0.5
                && (previous.height - rect.height).abs() <= 0.5
                && rect.x <= previous.x + previous.width + 0.5
            {
                let right = (previous.x + previous.width).max(rect.x + rect.width);
                previous.x = previous.x.min(rect.x);
                previous.width = right - previous.x;
                continue;
            }
            merged.push(rect);
        }
        if merged.is_empty() {
            return None;
        }
        Some(TextSelection {
            range: ordered,
            source_nodes: selected.iter().map(|(source, _, _)| *source).collect(),
            rects: merged,
            text,
        })
    }
}

#[derive(Clone, Debug)]
struct PreparedCommand<Id> {
    owners: Vec<Id>,
    command: PaintCmd,
}

#[derive(Clone, Debug)]
struct RetainedTextCluster<Id> {
    source: Id,
    range: Range<usize>,
    fragment: Fragment,
    rtl: bool,
}

struct SourceSpan<Id> {
    source: Option<Id>,
    owners: Vec<Id>,
    style: ComputedValues,
    range: Range<usize>,
    negative_margin_offset: f32,
}

#[derive(Clone)]
struct TextSource<Id> {
    source: Id,
    range: Range<usize>,
}

struct InlineAtom<Id> {
    source: Id,
    owners: Vec<Id>,
    index: usize,
    fragment: Fragment,
    line_width: f32,
    line_box_height: f32,
    margin_left: f32,
    margin_top: f32,
    edge: bool,
    paint: bool,
    vertical_align: VerticalAlign,
    font_size: f32,
    line_height: f32,
}

enum ShapedItem<Id> {
    Text(ShapedRun<Id>),
    InlineBox {
        source: Id,
        owners: Vec<Id>,
        fragment: Fragment,
        line_fragment: Fragment,
        edge: bool,
        paint: bool,
        line_y: f32,
    },
}

struct ShapedRun<Id> {
    source: Option<Id>,
    owners: Vec<Id>,
    line_baseline: f32,
    line_block_min: f32,
    line_block_max: f32,
    line_y: f32,
    fragment: Fragment,
    line_fragment: Fragment,
    font_instance: FontInstanceKey,
    font_size: f32,
    color: ColorF,
    glyphs: Vec<GlyphInstance>,
    clusters: Vec<ShapedCluster<Id>>,
}

#[derive(Clone)]
struct ShapedCluster<Id> {
    source: Id,
    range: Range<usize>,
    fragment: Fragment,
    rtl: bool,
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
        NodeKind::Element => styles.get(id).is_some_and(|style| {
            matches!(style.display, Display::Inline | Display::InlineBlock)
                && !matches!(style.position, Position::Absolute | Position::Fixed)
                && !(style.display == Display::Inline
                    && dom.dom_children(id).any(|child| {
                        !is_inline(dom, styles, child)
                            && !styles
                                .get(child)
                                .is_some_and(|child_style| child_style.display == Display::None)
                    }))
        }),
        _ => false,
    }
}

fn is_replaced_element<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy,
{
    dom.kind(id) == NodeKind::Element
        && dom.element_name(id).is_some_and(|name| {
            name.local.as_ref().eq_ignore_ascii_case("img")
                || name.local.as_ref().eq_ignore_ascii_case("canvas")
        })
}

struct BoxInlineCollector<'a, D, F>
where
    D: LayoutDom,
    F: FragmentLookup<D::NodeId>,
{
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    boxes: &'a CssBoxTree<D::NodeId>,
    fragments: &'a F,
    owners: &'a mut Vec<BoxId>,
    text: &'a mut String,
    spans: &'a mut Vec<SourceSpan<BoxId>>,
    inline_boxes: &'a mut Vec<InlineAtom<BoxId>>,
    negative_margin_offset: f32,
    percentage_basis: f32,
}

impl<D, F> BoxInlineCollector<'_, D, F>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    F: FragmentLookup<D::NodeId>,
{
    fn collect(&mut self, box_id: BoxId, inherited: &ComputedValues) {
        let css_box = &self.boxes[box_id];
        match css_box.origin {
            BoxOrigin::Text(node) => {
                let start = self.text.len();
                append_inline_text(self.text, self.dom.text(node).unwrap_or(""), inherited);
                if self.text.len() == start {
                    return;
                }
                self.spans.push(SourceSpan {
                    source: Some(box_id),
                    owners: self.owners.clone(),
                    style: inherited.clone(),
                    range: start..self.text.len(),
                    negative_margin_offset: self.negative_margin_offset,
                });
            },
            BoxOrigin::Element(node) => {
                let Some(style) = self.styles.get(node).cloned() else {
                    return;
                };
                let atomic = css_box.replaced
                    || (css_box.display.outside == Some(DisplayOutside::Inline)
                        && css_box.display.inside == Some(DisplayInside::FlowRoot));
                if atomic {
                    if let Some(fragment) = self.fragments.atomic_box_rect(box_id).copied() {
                        let font_size = super::paint::used_font_size(&style);
                        let (line_width, line_box_height, margin_left, margin_top) =
                            inline_margin_box(&style, fragment, font_size, self.percentage_basis);
                        self.inline_boxes.push(InlineAtom {
                            source: box_id,
                            owners: self.owners.clone(),
                            index: self.text.len(),
                            fragment,
                            line_width,
                            line_box_height,
                            margin_left,
                            margin_top,
                            edge: false,
                            paint: true,
                            vertical_align: style.vertical_align,
                            font_size,
                            line_height: super::layout::line_height_px(
                                &style.line_height,
                                font_size,
                            ),
                        });
                    }
                    return;
                }

                let ancestor_owners = self.owners.clone();
                let text_start = self.text.len();
                self.push_edge(box_id, &style, &ancestor_owners, true);
                let content_start = self.inline_boxes.len();
                let previous_negative_margin = self.negative_margin_offset;
                let leading_negative_margin = inline_margin_px(
                    style.margin_left,
                    super::paint::used_font_size(&style),
                    self.percentage_basis,
                )
                .min(0.0);
                self.negative_margin_offset += leading_negative_margin;
                let trailing_negative_margin = inline_margin_px(
                    style.margin_right,
                    super::paint::used_font_size(&style),
                    self.percentage_basis,
                )
                .min(0.0);
                self.owners.push(box_id);
                for child in css_box.children() {
                    self.collect(*child, &style);
                }
                self.owners.pop();
                self.negative_margin_offset =
                    previous_negative_margin + leading_negative_margin + trailing_negative_margin;
                let has_inline_content = self.inline_boxes.len() > content_start;
                self.push_edge(box_id, &style, &ancestor_owners, false);
                if self.text.len() == text_start && !has_inline_content {
                    self.push_empty_line_box(box_id, &style, &ancestor_owners);
                }
            },
            BoxOrigin::Anonymous { .. } => {
                for child in css_box.children() {
                    self.collect(*child, inherited);
                }
            },
            BoxOrigin::Pseudo { .. } => {},
        }
    }

    fn push_edge(&mut self, source: BoxId, style: &ComputedValues, owners: &[BoxId], start: bool) {
        let edges = inline_decoration_edges(style, self.percentage_basis);
        let em = super::paint::used_font_size(style);
        let margin = if start {
            inline_margin_px(style.margin_left, em, self.percentage_basis)
        } else {
            inline_margin_px(style.margin_right, em, self.percentage_basis)
        };
        let decoration = if start { edges.left } else { edges.right };
        let mut push = |width: f32, paint: bool| {
            if width.is_finite() && width.abs() > f32::EPSILON {
                self.inline_boxes.push(InlineAtom {
                    source,
                    owners: owners.to_vec(),
                    index: self.text.len(),
                    fragment: Fragment {
                        width,
                        ..Fragment::default()
                    },
                    line_width: width,
                    line_box_height: 0.0,
                    margin_left: 0.0,
                    margin_top: 0.0,
                    edge: true,
                    paint,
                    vertical_align: style.vertical_align,
                    font_size: em,
                    line_height: super::layout::line_height_px(&style.line_height, em),
                });
            }
        };
        if start {
            push(margin, false);
        }
        push(decoration, true);
        if !start {
            push(margin, false);
        }
    }

    fn push_empty_line_box(&mut self, source: BoxId, style: &ComputedValues, owners: &[BoxId]) {
        if matches!(style.line_height, CssLineHeight::Normal) {
            return;
        }
        let font_size = super::paint::used_font_size(style);
        let height = super::layout::line_height_px(&style.line_height, font_size);
        if height <= 0.0 {
            return;
        }
        self.inline_boxes.push(InlineAtom {
            source,
            owners: owners.to_vec(),
            index: self.text.len(),
            fragment: Fragment {
                width: 0.0,
                height,
                ..Fragment::default()
            },
            line_width: 0.0,
            line_box_height: height,
            margin_left: 0.0,
            margin_top: 0.0,
            edge: false,
            paint: false,
            vertical_align: style.vertical_align,
            font_size,
            line_height: height,
        });
    }
}

struct InlineCollector<'a, D, F>
where
    D: LayoutDom,
    F: FragmentLookup<D::NodeId>,
{
    dom: &'a D,
    styles: &'a StylePlane<D::NodeId>,
    fragments: &'a F,
    already_prepared: &'a HashSet<D::NodeId>,
    owners: &'a mut Vec<D::NodeId>,
    text: &'a mut String,
    spans: &'a mut Vec<SourceSpan<D::NodeId>>,
    inline_boxes: &'a mut Vec<InlineAtom<D::NodeId>>,
    negative_margin_offset: f32,
    percentage_basis: f32,
}

impl<D, F> InlineCollector<'_, D, F>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    F: FragmentLookup<D::NodeId>,
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
                    negative_margin_offset: self.negative_margin_offset,
                });
            },
            NodeKind::Element => {
                let Some(style) = self.styles.get(id).cloned() else {
                    return;
                };
                if style.display == Display::None {
                    return;
                }
                if is_replaced_element(self.dom, id) && style.display != Display::InlineBlock {
                    if let Some(fragment) = self.fragments.atomic_rect(id).copied() {
                        let font_size = super::paint::used_font_size(&style);
                        let (line_width, line_box_height, margin_left, margin_top) =
                            inline_margin_box(&style, fragment, font_size, self.percentage_basis);
                        self.inline_boxes.push(InlineAtom {
                            source: id,
                            owners: self.owners.clone(),
                            index: self.text.len(),
                            fragment,
                            line_width,
                            line_box_height,
                            margin_left,
                            margin_top,
                            edge: false,
                            paint: true,
                            vertical_align: style.vertical_align,
                            font_size,
                            line_height: super::layout::line_height_px(
                                &style.line_height,
                                font_size,
                            ),
                        });
                    }
                    return;
                }
                if style.display == Display::InlineBlock {
                    if let Some(fragment) = self.fragments.atomic_rect(id).copied() {
                        let font_size = super::paint::used_font_size(&style);
                        let (line_width, line_box_height, margin_left, margin_top) =
                            inline_margin_box(&style, fragment, font_size, self.percentage_basis);
                        self.inline_boxes.push(InlineAtom {
                            source: id,
                            owners: self.owners.clone(),
                            index: self.text.len(),
                            fragment,
                            line_width,
                            line_box_height,
                            margin_left,
                            margin_top,
                            edge: false,
                            paint: true,
                            vertical_align: style.vertical_align,
                            font_size: super::paint::used_font_size(&style),
                            line_height: super::layout::line_height_px(
                                &style.line_height,
                                super::paint::used_font_size(&style),
                            ),
                        });
                    }
                    return;
                }
                let ancestor_owners = self.owners.clone();
                let text_start = self.text.len();
                self.push_edge(id, &style, &ancestor_owners, true);
                let content_start = self.inline_boxes.len();
                let previous_negative_margin = self.negative_margin_offset;
                let leading_negative_margin = inline_margin_px(
                    style.margin_left,
                    super::paint::used_font_size(&style),
                    self.percentage_basis,
                )
                .min(0.0);
                self.negative_margin_offset += leading_negative_margin;
                let trailing_negative_margin = inline_margin_px(
                    style.margin_right,
                    super::paint::used_font_size(&style),
                    self.percentage_basis,
                )
                .min(0.0);
                self.owners.push(id);
                for child in self.dom.dom_children(id) {
                    if is_inline(self.dom, self.styles, child) {
                        self.collect(child, &style);
                    }
                }
                self.owners.pop();
                self.negative_margin_offset =
                    previous_negative_margin + leading_negative_margin + trailing_negative_margin;
                let has_inline_content = self.inline_boxes.len() > content_start;
                self.push_edge(id, &style, &ancestor_owners, false);
                if self.text.len() == text_start && !has_inline_content {
                    self.push_empty_line_box(id, &style, &ancestor_owners);
                }
            },
            _ => {},
        }
    }
}

impl<D, F> InlineCollector<'_, D, F>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    F: FragmentLookup<D::NodeId>,
{
    fn push_edge(
        &mut self,
        source: D::NodeId,
        style: &ComputedValues,
        owners: &[D::NodeId],
        start: bool,
    ) {
        let edges = inline_decoration_edges(style, self.percentage_basis);
        let em = super::paint::used_font_size(style);
        let margin = if start {
            inline_margin_px(style.margin_left, em, self.percentage_basis)
        } else {
            inline_margin_px(style.margin_right, em, self.percentage_basis)
        };
        let decoration = if start { edges.left } else { edges.right };
        let mut push = |width: f32, paint: bool| {
            if width.is_finite() && width.abs() > f32::EPSILON {
                self.inline_boxes.push(InlineAtom {
                    source,
                    owners: owners.to_vec(),
                    index: self.text.len(),
                    fragment: Fragment {
                        width,
                        ..Fragment::default()
                    },
                    line_width: width,
                    line_box_height: 0.0,
                    margin_left: 0.0,
                    margin_top: 0.0,
                    edge: true,
                    paint,
                    vertical_align: style.vertical_align,
                    font_size: em,
                    line_height: super::layout::line_height_px(&style.line_height, em),
                });
            }
        };
        if start {
            push(margin, false);
        }
        push(decoration, true);
        if !start {
            push(margin, false);
        }
    }

    fn push_empty_line_box(
        &mut self,
        source: D::NodeId,
        style: &ComputedValues,
        owners: &[D::NodeId],
    ) {
        if matches!(style.line_height, CssLineHeight::Normal) {
            return;
        }
        let font_size = super::paint::used_font_size(style);
        let height = super::layout::line_height_px(&style.line_height, font_size);
        if height <= 0.0 {
            return;
        }
        self.inline_boxes.push(InlineAtom {
            source,
            owners: owners.to_vec(),
            index: self.text.len(),
            fragment: Fragment {
                width: 0.0,
                height,
                ..Fragment::default()
            },
            line_width: 0.0,
            line_box_height: height,
            margin_left: 0.0,
            margin_top: 0.0,
            edge: false,
            paint: false,
            vertical_align: style.vertical_align,
            font_size,
            line_height: height,
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

fn inline_margin_px(value: Margin, em: f32, percentage_basis: f32) -> f32 {
    match value {
        Margin::Auto => 0.0,
        Margin::Value(value) => {
            super::layout::signed_length_percentage_px(value, em, percentage_basis)
        },
    }
}

fn inline_margin_box(
    style: &ComputedValues,
    fragment: Fragment,
    font_size: f32,
    percentage_basis: f32,
) -> (f32, f32, f32, f32) {
    let margin_left = inline_margin_px(style.margin_left, font_size, percentage_basis);
    let margin_right = inline_margin_px(style.margin_right, font_size, percentage_basis);
    let margin_top = inline_margin_px(style.margin_top, font_size, percentage_basis);
    let margin_bottom = inline_margin_px(style.margin_bottom, font_size, percentage_basis);
    (
        (fragment.width + margin_left + margin_right).max(0.0),
        (fragment.height + margin_top + margin_bottom).max(0.0),
        margin_left,
        margin_top,
    )
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
    Cow::Owned(collapse_css_whitespace(source))
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

    let leading = source.chars().next().is_some_and(is_css_whitespace);
    let trailing = source.chars().next_back().is_some_and(is_css_whitespace);
    if leading && !target.is_empty() && !target.ends_with(char::is_whitespace) {
        target.push(' ');
    }
    let collapsed = collapse_css_whitespace(source);
    if !collapsed.is_empty() {
        if !target.is_empty() && !target.ends_with(char::is_whitespace) && !leading {
            target.push(' ');
        }
        target.push_str(&collapsed);
    }
    if trailing && !target.is_empty() && !target.ends_with(char::is_whitespace) {
        target.push(' ');
    }
}

fn is_css_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}' | '\u{000A}' | '\u{000C}' | '\u{000D}' | ' '
    )
}

fn collapse_css_whitespace(source: &str) -> String {
    let mut output = String::new();
    let mut pending_space = false;
    for character in source.chars() {
        if is_css_whitespace(character) {
            pending_space = true;
            continue;
        }
        if pending_space && !output.is_empty() {
            output.push(' ');
        }
        pending_space = false;
        output.push(character);
    }
    if pending_space && !output.is_empty() {
        output.push(' ');
    }
    output
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
    if let Some(letter_spacing) = spacing_px(style.letter_spacing) {
        builder.push_default(StyleProperty::LetterSpacing(letter_spacing));
    }
    if let Some(word_spacing) = spacing_px(style.word_spacing) {
        builder.push_default(StyleProperty::WordSpacing(word_spacing));
    }
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
    builder.push(line_height(style), range.clone());
    if let Some(letter_spacing) = spacing_px(style.letter_spacing) {
        builder.push(StyleProperty::LetterSpacing(letter_spacing), range.clone());
    }
    if let Some(word_spacing) = spacing_px(style.word_spacing) {
        builder.push(StyleProperty::WordSpacing(word_spacing), range);
    }
}

fn text_alignment(style: TextAlign) -> Alignment {
    match style {
        TextAlign::Start => Alignment::Start,
        TextAlign::End => Alignment::End,
        TextAlign::Left => Alignment::Left,
        TextAlign::Right => Alignment::Right,
        TextAlign::Center => Alignment::Center,
        TextAlign::Justify => Alignment::Justify,
    }
}

fn vertical_align_shift(
    value: VerticalAlign,
    font_size: f32,
    line_box_height: f32,
    metrics: &parley::LineMetrics,
    item_y: f32,
    item_height: f32,
    is_inline_box: bool,
) -> f32 {
    match value {
        VerticalAlign::Baseline => 0.0,
        VerticalAlign::Sub => font_size * 0.2,
        VerticalAlign::Super => -font_size * 0.4,
        VerticalAlign::Length(value) => {
            -super::layout::signed_length_percentage_px(value, font_size, line_box_height)
        },
        VerticalAlign::Middle if is_inline_box => {
            metrics.baseline + font_size * 0.5 - (item_y + item_height * 0.5)
        },
        VerticalAlign::Top | VerticalAlign::TextTop if is_inline_box => {
            metrics.block_min_coord - item_y
        },
        VerticalAlign::Bottom | VerticalAlign::TextBottom if is_inline_box => {
            metrics.block_max_coord - (item_y + item_height)
        },
        VerticalAlign::Middle
        | VerticalAlign::Top
        | VerticalAlign::TextTop
        | VerticalAlign::Bottom
        | VerticalAlign::TextBottom => 0.0,
    }
}

fn spacing_px(spacing: Spacing) -> Option<f32> {
    match spacing {
        Spacing::Normal => None,
        Spacing::Length(length) => Some(length.to_px(16.0, 16.0, 0.0)),
    }
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

fn explicit_line_height(style: &ComputedValues) -> Option<f32> {
    if matches!(style.line_height, CssLineHeight::Normal) {
        None
    } else {
        Some(super::layout::line_height_px(
            &style.line_height,
            super::paint::used_font_size(style),
        ))
    }
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
