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
    LayoutContext, PositionedLayoutItem, StyleProperty,
};

use crate::{Fragment, FragmentPlane, StylePlane, paint::resolve_color};

type Brush = [f32; 4];

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
        let Some(parent_fragment) = fragments.get(parent) else {
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
                    (parent_fragment, parent_style),
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
            (parent_fragment, parent_style),
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
            style: style.clone(),
            range: 0..text.len(),
        }];
        for mut run in self.shape(text.as_ref(), &mut spans, fragment.width, style) {
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
        for root in roots {
            flatten_inline(
                dom,
                styles,
                *root,
                parent_style,
                &frame.prepared_sources,
                &mut text,
                &mut spans,
            );
        }
        if spans.is_empty() || text.is_empty() {
            return;
        }

        let origin = spans
            .iter()
            .filter_map(|span| span.source.and_then(|id| fragments.get(id)))
            .next()
            .map_or((parent_fragment.x, parent_fragment.y), |fragment| {
                (fragment.x, fragment.y)
            });
        for mut run in self.shape(&text, &mut spans, parent_fragment.width, parent_style) {
            let Some(source) = run.source else {
                continue;
            };
            for glyph in &mut run.glyphs {
                glyph.point.x += origin.0;
                glyph.point.y += origin.1;
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
        }
    }

    fn shape<Id>(
        &mut self,
        text: &str,
        spans: &mut [SourceSpan<Id>],
        width: f32,
        root_style: &ComputedValues,
    ) -> Vec<ShapedRun<Id>>
    where
        Id: Copy,
    {
        self.shape_count = self.shape_count.saturating_add(1);
        let mut builder =
            self.layout_context
                .ranged_builder(&mut self.font_context, text, 1.0, true);
        if let Some(first) = spans.first() {
            push_defaults(&mut builder, &first.style);
        }
        for span in spans.iter() {
            push_span(&mut builder, &span.style, span.range.clone());
        }
        let mut layout = builder.build(text);
        let wrap_width = (root_style.text_wrap_mode == TextWrapMode::Wrap)
            .then_some(width)
            .filter(|width| width.is_finite() && *width > 0.0);
        layout.break_all_lines(wrap_width);
        layout.align(Alignment::Start, AlignmentOptions::default());

        let mut result = Vec::new();
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let parley_run = run.run();
                let source = spans
                    .iter()
                    .find(|span| span.range.contains(&parley_run.text_range().start))
                    .and_then(|span| span.source);
                let font_instance = self.intern_font(parley_run.font());
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
                let [red, green, blue, alpha] = run.style().brush;
                result.push(ShapedRun {
                    source,
                    font_instance,
                    font_size: parley_run.font_size(),
                    color: ColorF::new(red, green, blue, alpha),
                    glyphs,
                });
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
    used_fonts: HashSet<FontInstanceKey>,
}

impl<Id> Default for TextFrame<Id> {
    fn default() -> Self {
        Self {
            prepared: HashMap::new(),
            prepared_sources: HashSet::new(),
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
}

struct SourceSpan<Id> {
    source: Option<Id>,
    style: ComputedValues,
    range: Range<usize>,
}

struct ShapedRun<Id> {
    source: Option<Id>,
    font_instance: FontInstanceKey,
    font_size: f32,
    color: ColorF,
    glyphs: Vec<GlyphInstance>,
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

fn flatten_inline<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    id: D::NodeId,
    inherited: &ComputedValues,
    already_prepared: &HashSet<D::NodeId>,
    text: &mut String,
    spans: &mut Vec<SourceSpan<D::NodeId>>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    match dom.kind(id) {
        NodeKind::Text => {
            if already_prepared.contains(&id) {
                return;
            }
            let start = text.len();
            append_inline_text(text, dom.text(id).unwrap_or(""), inherited);
            if text.len() == start {
                return;
            }
            spans.push(SourceSpan {
                source: Some(id),
                style: inherited.clone(),
                range: start..text.len(),
            });
        },
        NodeKind::Element => {
            let Some(style) = styles.get(id) else {
                return;
            };
            if style.display == Display::None {
                return;
            }
            for child in dom.dom_children(id) {
                if is_inline(dom, styles, child) {
                    flatten_inline(dom, styles, child, style, already_prepared, text, spans);
                }
            }
        },
        _ => {},
    }
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
    builder.push_default(StyleProperty::Brush(brush(style)));
    builder.push_default(line_height(style));
}

fn push_span(
    builder: &mut parley::RangedBuilder<'_, Brush>,
    style: &ComputedValues,
    range: Range<usize>,
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
    builder.push(StyleProperty::Brush(brush(style)), range.clone());
    builder.push(line_height(style), range);
}

fn brush(style: &ComputedValues) -> Brush {
    let color = resolve_color(style.color, ColorF::BLACK);
    [color.r, color.g, color.b, color.a]
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
