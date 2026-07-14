//! Clean-room Parley adapter for Livery text nodes.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use livery::{
    ComputedValues,
    values::{
        FontFamily as CssFontFamily, FontStyle as CssFontStyle, FontWeight as CssFontWeight,
        LineHeight as CssLineHeight, TextWrapMode,
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

use crate::{Fragment, paint::resolve_color};

type Brush = [f32; 4];

pub(crate) struct TextPainter {
    font_context: FontContext,
    layout_context: LayoutContext<Brush>,
    fonts: HashMap<FontInstanceKey, FontResource>,
    font_keys: HashMap<(u64, u32), FontInstanceKey>,
}

impl TextPainter {
    pub(crate) fn new() -> Self {
        Self {
            font_context: FontContext::new(),
            layout_context: LayoutContext::new(),
            fonts: HashMap::new(),
            font_keys: HashMap::new(),
        }
    }

    pub(crate) fn into_fonts(self) -> Vec<FontResource> {
        let mut fonts = self.fonts.into_values().collect::<Vec<_>>();
        fonts.sort_by_key(|font| (font.key.0.0, font.key.1));
        fonts
    }

    pub(crate) fn emit(
        &mut self,
        source: &str,
        style: &ComputedValues,
        fragment: &Fragment,
        commands: &mut Vec<PaintCmd>,
    ) {
        let text = normalized_text(source, style);
        if text.is_empty() {
            return;
        }

        let color = resolve_color(style.color, ColorF::BLACK);
        let brush = [color.r, color.g, color.b, color.a];
        let mut builder =
            self.layout_context
                .ranged_builder(&mut self.font_context, text.as_ref(), 1.0, true);
        builder.push_default(StyleProperty::FontSize(super::paint::used_font_size(style)));
        builder.push_default(font_family(style));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(font_weight(
            style,
        ))));
        builder.push_default(StyleProperty::FontStyle(font_style(style)));
        builder.push_default(StyleProperty::Brush(brush));
        builder.push_default(line_height(style));
        let mut layout = builder.build(text.as_ref());
        let wrap_width = (style.text_wrap_mode == TextWrapMode::Wrap).then_some(fragment.width);
        layout.break_all_lines(wrap_width);
        layout.align(Alignment::Start, AlignmentOptions::default());

        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let font = run.run().font();
                let key = self.intern_font(font);
                let glyphs = run
                    .positioned_glyphs()
                    .map(|glyph| GlyphInstance {
                        index: glyph.id,
                        point: LayoutPoint::new(fragment.x + glyph.x, fragment.y + glyph.y),
                    })
                    .collect::<Vec<_>>();
                if glyphs.is_empty() {
                    continue;
                }
                let [red, green, blue, alpha] = run.style().brush;
                commands.push(PaintCmd::DrawText(TextRunItem {
                    placement: CommonPlacement::new(super::paint::bounds(fragment)),
                    font_instance: key,
                    font_size: run.run().font_size(),
                    color: ColorF::new(red, green, blue, alpha),
                    glyphs,
                    options: TextOptions::default(),
                }));
            }
        }
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

fn normalized_text<'a>(source: &'a str, style: &ComputedValues) -> Cow<'a, str> {
    use livery::values::WhiteSpaceCollapse;

    if style.white_space_collapse == WhiteSpaceCollapse::Preserve {
        return Cow::Borrowed(source);
    }
    let collapsed = source.split_whitespace().collect::<Vec<_>>().join(" ");
    Cow::Owned(collapsed)
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
