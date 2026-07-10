/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Inline-span flattening + parley wrapper.
//!
//! Two responsibilities:
//!
//! 1. **Flatten** a `Vec<inker::InlineSpan>` into a single text buffer plus
//!    ranged style attributes plus link metadata. parley wants byte ranges
//!    over a single string; we walk the inline tree to produce them.
//! 2. **Lay out** the flattened text via parley, emitting our own
//!    [`GlyphRun`] records (positions in packet-local coordinates) plus
//!    interaction regions for links.

use std::ops::Range;

use inker::InlineSpan;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, GenericFamily, LayoutContext, LineHeight,
    PositionedLayoutItem, StyleProperty,
};

use crate::font_table::FontInterner;
use crate::style::InlineStyle;
use crate::style_sheet::{LinkAdornment, WrapPolicy};
use crate::types::{
    GlyphRun, InteractionKind, InteractionRegion, Point, PositionedGlyph, Rect, Size, TextStyle,
};

/// Block-level base style for a text block (heading vs body vs code etc.).
#[derive(Clone, Debug)]
pub struct TextBaseStyle {
    pub font_size: f32,
    pub font_family: String,
    pub bold: bool,
    pub italic: bool,
    pub monospace: bool,
    pub line_height_ratio: f32,
    /// Premultiplied RGBA (0..=1) for the block's text. Inline links + code
    /// override it per-run inside [`layout_text_block`].
    pub color: [f32; 4],
    /// Whether the block wraps to the available width (`Wrap`) or lays out on
    /// its natural width and overflows for the host to scroll (`NoWrap`, e.g.
    /// code blocks).
    pub wrap: WrapPolicy,
}

impl Default for TextBaseStyle {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_family: "system-ui".to_string(),
            bold: false,
            italic: false,
            monospace: false,
            line_height_ratio: 1.4,
            color: [0.0, 0.0, 0.0, 1.0],
            wrap: WrapPolicy::Wrap,
        }
    }
}

/// Flatten a span tree into a single text string plus ranged styles plus
/// link annotations. Returned ranges are byte offsets into the text string.
///
/// `adornment` + `base_scheme` drive the per-link prefix glyph (the `⇒` / `⇗`
/// scheme arrows): when adornment applies, the prefix is prepended to the
/// link's display text, styled as part of the link and covered by its byte
/// range (so it colors + hit-tests as the link).
pub fn flatten_inline(
    spans: &[InlineSpan],
    adornment: LinkAdornment,
    base_scheme: Option<&str>,
) -> Flattened {
    let mut out = Flattened::default();
    flatten_into(spans, InlineStyle::NORMAL, adornment, base_scheme, &mut out);
    out
}

#[derive(Clone, Debug, Default)]
pub struct Flattened {
    pub text: String,
    /// Per-span style ranges (byte offsets). May overlap if you nest
    /// emphasis-in-strong; later ranges win in the parley range stack
    /// (parley merges them additively per-property).
    pub styles: Vec<(Range<usize>, InlineStyle)>,
    /// Link annotations. Byte range of the link's display text + the URL.
    pub links: Vec<(Range<usize>, String)>,
}

fn flatten_into(
    spans: &[InlineSpan],
    inherited: InlineStyle,
    adornment: LinkAdornment,
    base_scheme: Option<&str>,
    out: &mut Flattened,
) {
    for span in spans {
        match span {
            InlineSpan::Text(t) => {
                let start = out.text.len();
                out.text.push_str(t);
                let end = out.text.len();
                if start < end {
                    out.styles.push((start..end, inherited));
                }
            }
            InlineSpan::Code(t) => {
                let start = out.text.len();
                out.text.push_str(t);
                let end = out.text.len();
                if start < end {
                    out.styles.push((start..end, inherited.with_monospace()));
                }
            }
            InlineSpan::Emphasis(inner) => {
                flatten_into(inner, inherited.with_italic(), adornment, base_scheme, out);
            }
            InlineSpan::Strong(inner) => {
                flatten_into(inner, inherited.with_bold(), adornment, base_scheme, out);
            }
            InlineSpan::Link {
                url, spans: inner, ..
            } => {
                let link_start = out.text.len();
                // Scheme-arrow prefix, styled + ranged as part of the link.
                if let Some(prefix) = adornment.prefix_for(url, base_scheme) {
                    let p_start = out.text.len();
                    out.text.push_str(prefix);
                    out.styles
                        .push((p_start..out.text.len(), inherited.with_link()));
                }
                flatten_into(inner, inherited.with_link(), adornment, base_scheme, out);
                let link_end = out.text.len();
                if link_start < link_end {
                    out.links.push((link_start..link_end, url.clone()));
                }
            }
            InlineSpan::SoftBreak => {
                out.text.push(' ');
            }
            InlineSpan::LineBreak => {
                out.text.push('\n');
            }
        }
    }
}

/// Per-block layout output.
#[derive(Clone, Debug)]
pub struct LaidOutText {
    pub glyph_runs: Vec<GlyphRun>,
    pub total_size: Size,
    pub interactions: Vec<InteractionRegion>,
}

/// Owns parley contexts so consumers don't recreate them per call.
pub struct LayoutEnvironment {
    pub font_cx: FontContext,
    pub layout_cx: LayoutContext<InlineStyle>,
}

impl Default for LayoutEnvironment {
    fn default() -> Self {
        Self {
            font_cx: FontContext::new(),
            layout_cx: LayoutContext::new(),
        }
    }
}

impl LayoutEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a `LayoutEnvironment` with a font resolver pre-registered
    /// against parley's font context. Layout-time text shaping then sees
    /// whatever fonts the resolver provides. The render side no longer
    /// consults the resolver: glyph runs carry parley's actually-shaped
    /// face (a [`FontFaceId`](crate::FontFaceId) into the font sidecar),
    /// so the bytes downstream renders come from the same face parley
    /// shaped against.
    pub fn with_resolver<R: crate::font::FontResolver>(resolver: &R) -> Self {
        let mut env = Self::default();
        resolver.register_with_parley(&mut env.font_cx);
        env
    }
}

/// Lay out a single text block via parley. `origin` is the block's
/// top-left in packet-local coordinates; emitted glyph runs and
/// interaction regions are translated into that frame.
pub fn layout_text_block(
    env: &mut LayoutEnvironment,
    flattened: &Flattened,
    base: &TextBaseStyle,
    link_color: [f32; 4],
    code_color: [f32; 4],
    available_width: f32,
    origin: Point,
    fonts: &mut FontInterner,
) -> LaidOutText {
    let display_scale = 1.0_f32;
    let quantize = true;

    let mut builder =
        env.layout_cx
            .ranged_builder(&mut env.font_cx, &flattened.text, display_scale, quantize);

    // Block-level defaults: font family, size, line height, weight, style.
    let body_family = if base.monospace {
        FontFamily::from(GenericFamily::Monospace)
    } else {
        FontFamily::from(GenericFamily::SystemUi)
    };
    builder.push_default(body_family);
    builder.push_default(StyleProperty::FontSize(base.font_size));
    builder.push_default(LineHeight::FontSizeRelative(base.line_height_ratio));
    if base.bold {
        builder.push_default(StyleProperty::FontWeight(parley::FontWeight::BOLD));
    }
    if base.italic {
        builder.push_default(StyleProperty::FontStyle(parley::FontStyle::Italic));
    }
    // Brush default — parley needs a default brush even if we don't paint.
    builder.push_default(StyleProperty::Brush(InlineStyle::NORMAL));

    // Per-range styling: walk the flattened ranges and push only the
    // attributes each range adds beyond the block-level base.
    for (range, style) in &flattened.styles {
        if style.bold && !base.bold {
            builder.push(
                StyleProperty::FontWeight(parley::FontWeight::BOLD),
                range.clone(),
            );
        }
        if style.italic && !base.italic {
            builder.push(
                StyleProperty::FontStyle(parley::FontStyle::Italic),
                range.clone(),
            );
        }
        if style.monospace && !base.monospace {
            builder.push(FontFamily::from(GenericFamily::Monospace), range.clone());
        }
        // Links don't have an explicit visual style here; the
        // InteractionRegion carries the URL. A future slice could push an
        // Underline + colored brush for the link range.
        builder.push(StyleProperty::Brush(*style), range.clone());
    }

    let mut layout = builder.build(&flattened.text);
    // `NoWrap` roles (e.g. code blocks) lay out on their natural width and
    // overflow horizontally for the host to scroll; `Wrap` constrains to the
    // available content width. Explicit line breaks are kept either way.
    let wrap_width = match base.wrap {
        WrapPolicy::Wrap => Some(available_width),
        WrapPolicy::NoWrap => None,
    };
    layout.break_all_lines(wrap_width);
    layout.align(Alignment::Start, AlignmentOptions::default());

    // Walk lines, accumulating Y positions ourselves (parley gives us
    // baseline + line_height per line; the line's top is baseline minus
    // the metrics ascent, which we approximate via cumulative line height).
    let mut glyph_runs: Vec<GlyphRun> = Vec::new();
    let mut interactions: Vec<InteractionRegion> = Vec::new();
    let mut line_top_y = 0.0_f32;

    for line in layout.lines() {
        let line_metrics = line.metrics();
        let line_height = line_metrics.line_height;
        let baseline_in_line = line_metrics.baseline - line_top_y;

        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(parley_run) = item else {
                continue;
            };
            let run = parley_run.run();
            let font_size = run.font_size();
            let advance_x = parley_run.offset();
            let baseline_y = parley_run.baseline();

            let mut glyphs: Vec<PositionedGlyph> = Vec::new();
            for glyph in parley_run.positioned_glyphs() {
                glyphs.push(PositionedGlyph {
                    glyph_id: glyph.id,
                    x: glyph.x - advance_x,
                    y: glyph.y - baseline_y,
                    advance: glyph.advance,
                });
            }

            let run_origin = Point::new(origin.x + advance_x, origin.y + line_top_y);

            // Source the face from parley's actual choice (after any
            // fallback), not from the requested attrs — the glyph ids
            // above index into *this* face. `font_attrs` below is the
            // *requested* family/weight/style, kept only for a11y/debug.
            let font_face = fonts.intern(run.font());

            let attrs = run.font_attrs();
            let weight_value = attrs.weight.value() as u16;
            let font_style = if matches!(attrs.style, parley::FontStyle::Italic) {
                TextStyle::Italic
            } else {
                TextStyle::Normal
            };

            // Per-run color by brush role: links and inline code carry their
            // own token colors; everything else paints in the block's base
            // color. parley already segments runs at brush boundaries, so a
            // link / inline-code span is its own run here.
            let brush = parley_run.style().brush;
            let run_color = if brush.link {
                link_color
            } else if brush.monospace {
                code_color
            } else {
                base.color
            };

            glyph_runs.push(GlyphRun {
                origin: run_origin,
                font_size,
                font_face,
                font_family: family_label_from_brush(brush, base),
                font_weight: weight_value,
                font_style,
                glyphs,
                baseline_y: baseline_in_line,
                color: run_color,
            });
        }

        // Link interaction regions: walk the line's text range, intersect
        // with each link's byte range, emit a rect for the matched segment.
        let line_text_range = line.text_range();
        for (link_range, url) in &flattened.links {
            let intersect_start = link_range.start.max(line_text_range.start);
            let intersect_end = link_range.end.min(line_text_range.end);
            if intersect_start >= intersect_end {
                continue;
            }
            let mut min_x = f32::MAX;
            let mut max_x = f32::MIN;
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(parley_run) = item else {
                    continue;
                };
                let run_text_range = parley_run.run().text_range();
                let run_intersect_start = intersect_start.max(run_text_range.start);
                let run_intersect_end = intersect_end.min(run_text_range.end);
                if run_intersect_start >= run_intersect_end {
                    continue;
                }
                let advance_x = parley_run.offset();
                // v1 approximation: if any portion of this run is in the
                // link range, include the whole run's advance. A future
                // slice can do per-cluster geometry.
                min_x = min_x.min(advance_x);
                let run_advance: f32 = parley_run.positioned_glyphs().map(|g| g.advance).sum();
                max_x = max_x.max(advance_x + run_advance);
            }
            if min_x < max_x {
                interactions.push(InteractionRegion {
                    bounds: Rect::from_xywh(
                        origin.x + min_x,
                        origin.y + line_top_y,
                        max_x - min_x,
                        line_height,
                    ),
                    kind: InteractionKind::Link { url: url.clone() },
                });
            }
        }

        line_top_y += line_height;
    }

    let total_height = layout.height();
    let total_width = layout.width();

    LaidOutText {
        glyph_runs,
        total_size: Size::new(total_width, total_height),
        interactions,
    }
}

/// Resolve a friendly family-name label for a glyph run. parley's brush
/// type carries our `InlineStyle`; we use that to pick body vs mono. The
/// real font face used by parley is opaque to consumers — they look up
/// concrete fonts via their own font cache when rendering.
fn family_label_from_brush(brush: InlineStyle, base: &TextBaseStyle) -> String {
    if brush.monospace || base.monospace {
        "monospace".to_string()
    } else {
        base.font_family.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str) -> InlineSpan {
        InlineSpan::Link {
            url: url.into(),
            title: None,
            spans: vec![InlineSpan::Text("label".into())],
            predicate: None,
        }
    }

    #[test]
    fn no_adornment_leaves_link_text_unprefixed() {
        let f = flatten_inline(&[link("gemini://x/")], LinkAdornment::None, Some("gemini"));
        assert_eq!(f.text, "label");
        assert_eq!(f.links.len(), 1);
        assert_eq!(f.links[0].0, 0..5);
    }

    #[test]
    fn in_protocol_link_gets_rightwards_double_arrow() {
        let f = flatten_inline(
            &[link("gemini://x/")],
            LinkAdornment::SchemeArrow,
            Some("gemini"),
        );
        assert!(f.text.starts_with("\u{21d2} "), "got {:?}", f.text);
        // The arrow is part of the link: the link byte range covers the whole
        // "⇒ label" string.
        assert_eq!(f.links.len(), 1);
        assert_eq!(&f.text[f.links[0].0.clone()], f.text.as_str());
    }

    #[test]
    fn external_link_gets_northeast_double_arrow() {
        let f = flatten_inline(
            &[link("https://x/")],
            LinkAdornment::SchemeArrow,
            Some("gemini"),
        );
        assert!(f.text.starts_with("\u{21d7} "), "got {:?}", f.text);
    }

    #[test]
    fn relative_link_is_in_protocol() {
        let f = flatten_inline(&[link("/page")], LinkAdornment::SchemeArrow, Some("gemini"));
        assert!(f.text.starts_with("\u{21d2} "), "got {:?}", f.text);
    }

    #[test]
    fn adornment_prefix_carries_link_style() {
        let f = flatten_inline(
            &[link("https://x/")],
            LinkAdornment::SchemeArrow,
            Some("gemini"),
        );
        // Every style range over the link (the arrow prefix + the label) is a
        // link, and one of them begins at byte 0 (the prefix).
        assert!(f.styles.iter().all(|(_, s)| s.link), "all link-styled");
        assert!(
            f.styles.iter().any(|(r, _)| r.start == 0),
            "prefix styled from 0"
        );
    }
}
