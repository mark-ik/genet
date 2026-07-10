/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Top-level document layout — dispatches per [`Block`] variant
//! and stacks the resulting blocks vertically inside the viewport.
//!
//! v1 is a simple top-down stack with no float / no inline-image flow /
//! no scrolling. Width fills the available content width; height grows
//! to fit content (may exceed `viewport.height`).

use inker::{Block, EngineDocument, InlineSpan};

use crate::font_table::{FontInterner, FontTable};
use crate::style_sheet::{BlockRole, ColorToken, DocumentStyleSheet, ResolvedBlockStyle};
use crate::text::{
    Flattened, LaidOutText, LayoutEnvironment, TextBaseStyle, flatten_inline, layout_text_block,
};
use crate::types::{
    DocumentRenderPacket, InteractionRegion, Point, Rect, RenderedBlock, RenderedBlockKind, Size,
    Viewport,
};

/// A laid-out document: the serializable [`DocumentRenderPacket`] plus the
/// out-of-band [`FontTable`] sidecar carrying the `parley::FontData` each
/// `GlyphRun` was shaped against. The packet alone is the portable,
/// serializable shape; `fonts` holds `Arc`-cheap face handles whose owned
/// bytes materialize only at the `paint_list_api` boundary (the
/// `PaintList`'s font side-table). See [`crate::font_table`].
#[derive(Clone, Debug)]
pub struct LaidOutDocument {
    pub packet: DocumentRenderPacket,
    pub fonts: FontTable,
}

/// Layout entry point. Consumes an `EngineDocument` and a viewport,
/// produces a [`LaidOutDocument`]: a portable [`DocumentRenderPacket`]
/// downstream renderers paint, plus the font sidecar that resolves each
/// run's [`FontFaceId`](crate::FontFaceId) to the real face bytes parley
/// shaped against.
pub fn layout_document(
    document: &EngineDocument,
    viewport: Viewport,
    style: &DocumentStyleSheet,
) -> LaidOutDocument {
    let mut env = LayoutEnvironment::new();
    // The document's own scheme classifies its links as in-protocol vs
    // external for the `⇒` / `⇗` adornment.
    let base_scheme = crate::style_sheet::url_scheme(&document.address).map(str::to_string);
    let mut layouter = DocumentLayouter::new(viewport, style, &mut env, base_scheme);

    for (idx, block) in document.blocks.iter().enumerate() {
        layouter.lay_out_block(block, idx, 0);
    }

    layouter.finish()
}

/// Build the parley block base from a resolved role style: the role's
/// typography, base text `color`, and `wrap` policy.
fn text_base_from(resolved: &ResolvedBlockStyle) -> TextBaseStyle {
    TextBaseStyle {
        font_size: resolved.font_size,
        font_family: resolved.font_family.clone(),
        bold: resolved.bold,
        italic: resolved.italic,
        monospace: resolved.monospace,
        line_height_ratio: resolved.line_height_ratio,
        color: resolved.color,
        wrap: resolved.wrap,
    }
}

struct DocumentLayouter<'a> {
    viewport: Viewport,
    style: &'a DocumentStyleSheet,
    env: &'a mut LayoutEnvironment,
    cursor_y: f32,
    blocks: Vec<RenderedBlock>,
    interactions: Vec<InteractionRegion>,
    max_x: f32,
    /// Interns parley's chosen face per run; sealed into the
    /// [`LaidOutDocument`]'s [`FontTable`] sidecar at `finish`.
    fonts: FontInterner,
    /// The document's own URL scheme, for classifying links (in-protocol vs
    /// external) when adorning them. `None` for schemeless addresses.
    base_scheme: Option<String>,
}

impl<'a> DocumentLayouter<'a> {
    fn new(
        viewport: Viewport,
        style: &'a DocumentStyleSheet,
        env: &'a mut LayoutEnvironment,
        base_scheme: Option<String>,
    ) -> Self {
        Self {
            viewport,
            style,
            env,
            cursor_y: style.vertical_padding,
            blocks: Vec::new(),
            interactions: Vec::new(),
            max_x: 0.0,
            fonts: FontInterner::new(),
            base_scheme,
        }
    }

    fn content_left(&self, indent_level: u32) -> f32 {
        self.style.horizontal_padding + (indent_level as f32) * self.style.indent_per_level
    }

    fn available_width(&self, indent_level: u32) -> f32 {
        let used = self.content_left(indent_level) + self.style.horizontal_padding;
        (self.viewport.width - used).max(0.0)
    }

    fn lay_out_block(&mut self, block: &Block, source_index: usize, indent_level: u32) {
        let rendered = self.render_block(block, source_index, indent_level);
        if let Some(rendered) = rendered {
            self.cursor_y = rendered.bounds.max_y();
            self.max_x = self.max_x.max(rendered.bounds.max_x());
            self.blocks.push(rendered);
        }
    }

    fn render_block(
        &mut self,
        block: &Block,
        source_index: usize,
        indent_level: u32,
    ) -> Option<RenderedBlock> {
        match block {
            // Tables render through the serval note path, not document-canvas
            // (cards are uniform thumbnails); skipped here until a card
            // table-preview lands, and unreachable until parsers emit Table.
            Block::Table { .. } => None,
            Block::Heading { level, spans } => {
                Some(self.render_heading(source_index, indent_level, *level, spans))
            }
            Block::Paragraph { spans } => {
                let resolved = self.style.resolve(BlockRole::Body);
                Some(self.render_paragraph(
                    source_index,
                    indent_level,
                    spans,
                    text_base_from(&resolved),
                    resolved.spacing_below,
                ))
            }
            Block::CodeBlock { text, .. } => {
                Some(self.render_code_block(source_index, indent_level, text))
            }
            Block::Preformatted { text } => {
                Some(self.render_code_block(source_index, indent_level, text))
            }
            Block::Quote { blocks } => {
                Some(self.render_group(source_index, indent_level + 1, blocks))
            }
            Block::List { items, .. } => {
                Some(self.render_list(source_index, indent_level + 1, items))
            }
            Block::Image { url, alt } => {
                Some(self.render_image(source_index, indent_level, url.clone(), alt.clone()))
            }
            Block::Rule => Some(self.render_rule(source_index, indent_level)),
            Block::FeedHeader {
                title,
                subtitle,
                summary,
                source_url,
            } => Some(self.render_feed_header(
                source_index,
                indent_level,
                title,
                subtitle.as_deref(),
                summary.as_deref(),
                source_url.as_deref(),
            )),
            Block::FeedEntry {
                title,
                date,
                summary,
                article_url,
                source_url,
            } => Some(self.render_feed_entry(
                source_index,
                indent_level,
                title,
                date.as_deref(),
                summary.as_deref(),
                article_url.as_deref(),
                source_url.as_deref(),
            )),
            Block::MetadataRow { label, value } => {
                Some(self.render_metadata_row(source_index, indent_level, label, value))
            }
            Block::Badge { text } => Some(self.render_badge(source_index, indent_level, text)),
        }
    }

    // -------------------------------------------------------------------
    // Block renderers
    // -------------------------------------------------------------------

    fn render_heading(
        &mut self,
        source_index: usize,
        indent_level: u32,
        level: u8,
        spans: &[InlineSpan],
    ) -> RenderedBlock {
        let resolved = self.style.resolve(BlockRole::Heading(level));
        let base = text_base_from(&resolved);
        self.render_text_block_with_spacing(
            source_index,
            indent_level,
            spans,
            base,
            resolved.spacing_above,
            resolved.spacing_below,
        )
    }

    fn render_paragraph(
        &mut self,
        source_index: usize,
        indent_level: u32,
        spans: &[InlineSpan],
        base: TextBaseStyle,
        spacing_below: f32,
    ) -> RenderedBlock {
        self.render_text_block_with_spacing(
            source_index,
            indent_level,
            spans,
            base,
            0.0,
            spacing_below,
        )
    }

    fn render_text_block_with_spacing(
        &mut self,
        source_index: usize,
        indent_level: u32,
        spans: &[InlineSpan],
        base: TextBaseStyle,
        spacing_above: f32,
        spacing_below: f32,
    ) -> RenderedBlock {
        let flattened = flatten_inline(
            spans,
            self.style.link_adornment,
            self.base_scheme.as_deref(),
        );
        self.render_flattened_with_spacing(
            source_index,
            indent_level,
            &flattened,
            base,
            spacing_above,
            spacing_below,
        )
    }

    fn render_flattened_with_spacing(
        &mut self,
        source_index: usize,
        indent_level: u32,
        flattened: &Flattened,
        base: TextBaseStyle,
        spacing_above: f32,
        spacing_below: f32,
    ) -> RenderedBlock {
        let origin = Point::new(
            self.content_left(indent_level),
            self.cursor_y + spacing_above,
        );
        let available = self.available_width(indent_level);
        // Inline link + code colors are sheet-global (any block can contain
        // them); the block's base color rides on `base`.
        let link_color = self.style.token_color(ColorToken::LinkText);
        let code_color = self.style.token_color(ColorToken::CodeText);
        let LaidOutText {
            glyph_runs,
            total_size,
            mut interactions,
        } = layout_text_block(
            self.env,
            flattened,
            &base,
            link_color,
            code_color,
            available,
            origin,
            &mut self.fonts,
        );

        self.interactions.append(&mut interactions);

        let bounds = Rect::new(
            origin,
            Size::new(total_size.width, total_size.height + spacing_below),
        );

        RenderedBlock {
            source_block_index: source_index,
            bounds,
            kind: RenderedBlockKind::Text { glyph_runs },
        }
    }

    fn render_code_block(
        &mut self,
        source_index: usize,
        indent_level: u32,
        text: &str,
    ) -> RenderedBlock {
        let resolved = self.style.resolve(BlockRole::Code);
        let base = text_base_from(&resolved);
        let spans = vec![InlineSpan::Text(text.to_string())];
        self.render_text_block_with_spacing(
            source_index,
            indent_level,
            &spans,
            base,
            resolved.spacing_above,
            resolved.spacing_below,
        )
    }

    fn render_group(
        &mut self,
        source_index: usize,
        indent_level: u32,
        children: &[Block],
    ) -> RenderedBlock {
        let group_top = self.cursor_y;
        let mut child_blocks: Vec<RenderedBlock> = Vec::new();
        for (i, child) in children.iter().enumerate() {
            // Children carry their own source indices in the parent's
            // coordinate; we project the parent's index into a synthetic
            // sub-index space (parent_index * 1000 + child_index). Crude
            // but stable enough for v1 hit-back-to-source mapping.
            let synthetic = source_index.saturating_mul(1000) + i;
            if let Some(rendered) = self.render_block(child, synthetic, indent_level) {
                self.cursor_y = rendered.bounds.max_y();
                self.max_x = self.max_x.max(rendered.bounds.max_x());
                child_blocks.push(rendered);
            }
        }
        let group_bottom = self.cursor_y;
        let group_left = self.content_left(indent_level);
        let group_right = self.max_x;
        RenderedBlock {
            source_block_index: source_index,
            bounds: Rect::from_xywh(
                group_left,
                group_top,
                (group_right - group_left).max(0.0),
                group_bottom - group_top,
            ),
            kind: RenderedBlockKind::Group {
                children: child_blocks,
            },
        }
    }

    fn render_list(
        &mut self,
        source_index: usize,
        indent_level: u32,
        items: &[Vec<Block>],
    ) -> RenderedBlock {
        let group_top = self.cursor_y;
        let mut child_blocks: Vec<RenderedBlock> = Vec::new();
        for (i, item) in items.iter().enumerate() {
            for (j, child) in item.iter().enumerate() {
                let synthetic = source_index.saturating_mul(1000) + i.saturating_mul(100) + j;
                if let Some(rendered) = self.render_block(child, synthetic, indent_level) {
                    self.cursor_y = rendered.bounds.max_y();
                    self.max_x = self.max_x.max(rendered.bounds.max_x());
                    child_blocks.push(rendered);
                }
            }
        }
        let group_bottom = self.cursor_y;
        let group_left = self.content_left(indent_level);
        let group_right = self.max_x;
        RenderedBlock {
            source_block_index: source_index,
            bounds: Rect::from_xywh(
                group_left,
                group_top,
                (group_right - group_left).max(0.0),
                group_bottom - group_top,
            ),
            kind: RenderedBlockKind::Group {
                children: child_blocks,
            },
        }
    }

    fn render_image(
        &mut self,
        source_index: usize,
        indent_level: u32,
        url: String,
        alt: String,
    ) -> RenderedBlock {
        // v1 reserves a placeholder strip the height of one line of body
        // text. Renderer fetches + paints the actual image; document-canvas
        // doesn't load bytes.
        let line_height = self.style.line_height(self.style.body_font_size);
        let height = line_height * 6.0; // ~6 lines worth of placeholder
        let origin = Point::new(self.content_left(indent_level), self.cursor_y);
        let bounds = Rect::new(
            origin,
            Size::new(
                self.available_width(indent_level),
                height + self.style.block_spacing(),
            ),
        );
        RenderedBlock {
            source_block_index: source_index,
            bounds,
            kind: RenderedBlockKind::Image { url, alt },
        }
    }

    fn render_rule(&mut self, source_index: usize, indent_level: u32) -> RenderedBlock {
        let origin = Point::new(self.content_left(indent_level), self.cursor_y);
        let bounds = Rect::new(
            origin,
            Size::new(
                self.available_width(indent_level),
                self.style.block_spacing(),
            ),
        );
        RenderedBlock {
            source_block_index: source_index,
            bounds,
            kind: RenderedBlockKind::Rule,
        }
    }

    fn render_feed_header(
        &mut self,
        source_index: usize,
        indent_level: u32,
        title: &str,
        subtitle: Option<&str>,
        summary: Option<&str>,
        source_url: Option<&str>,
    ) -> RenderedBlock {
        let mut composed: Vec<Block> = Vec::new();
        composed.push(Block::Heading {
            level: 1,
            spans: vec![InlineSpan::Text(title.to_string())],
        });
        if let Some(s) = subtitle {
            composed.push(Block::Heading {
                level: 2,
                spans: vec![InlineSpan::Text(s.to_string())],
            });
        }
        if let Some(s) = summary {
            composed.push(Block::Paragraph {
                spans: vec![InlineSpan::Text(s.to_string())],
            });
        }
        if let Some(url) = source_url {
            composed.push(Block::Paragraph {
                spans: vec![InlineSpan::Link {
                    url: url.to_string(),
                    title: None,
                    spans: vec![InlineSpan::Text("Open source".to_string())],
                    predicate: None,
                }],
            });
        }
        self.render_group(source_index, indent_level, &composed)
    }

    fn render_feed_entry(
        &mut self,
        source_index: usize,
        indent_level: u32,
        title: &str,
        date: Option<&str>,
        summary: Option<&str>,
        article_url: Option<&str>,
        source_url: Option<&str>,
    ) -> RenderedBlock {
        let mut composed: Vec<Block> = Vec::new();
        composed.push(Block::Heading {
            level: 2,
            spans: vec![InlineSpan::Text(title.to_string())],
        });
        if let Some(d) = date {
            composed.push(Block::Paragraph {
                spans: vec![InlineSpan::Emphasis(vec![InlineSpan::Text(d.to_string())])],
            });
        }
        if let Some(s) = summary {
            composed.push(Block::Paragraph {
                spans: vec![InlineSpan::Text(s.to_string())],
            });
        }
        if let Some(url) = article_url {
            composed.push(Block::Paragraph {
                spans: vec![InlineSpan::Link {
                    url: url.to_string(),
                    title: None,
                    spans: vec![InlineSpan::Text("Open article".to_string())],
                    predicate: None,
                }],
            });
        }
        if let Some(url) = source_url {
            composed.push(Block::Paragraph {
                spans: vec![InlineSpan::Link {
                    url: url.to_string(),
                    title: None,
                    spans: vec![InlineSpan::Text("Open source".to_string())],
                    predicate: None,
                }],
            });
        }
        self.render_group(source_index, indent_level, &composed)
    }

    fn render_metadata_row(
        &mut self,
        source_index: usize,
        indent_level: u32,
        label: &str,
        value: &str,
    ) -> RenderedBlock {
        // Label in bold + value in normal. Lay out as a single paragraph.
        let spans = vec![
            InlineSpan::Strong(vec![InlineSpan::Text(format!("{label}: "))]),
            InlineSpan::Text(value.to_string()),
        ];
        let resolved = self.style.resolve(BlockRole::Metadata);
        let base = text_base_from(&resolved);
        self.render_text_block_with_spacing(
            source_index,
            indent_level,
            &spans,
            base,
            resolved.spacing_above,
            resolved.spacing_below,
        )
    }

    fn render_badge(
        &mut self,
        source_index: usize,
        indent_level: u32,
        text: &str,
    ) -> RenderedBlock {
        // Badge as a small italic paragraph; renderer paints the pill if it
        // wants. v1 doesn't carry pill-shape metadata.
        let spans = vec![InlineSpan::Emphasis(vec![InlineSpan::Text(
            text.to_string(),
        )])];
        let resolved = self.style.resolve(BlockRole::Badge);
        let base = text_base_from(&resolved);
        self.render_text_block_with_spacing(
            source_index,
            indent_level,
            &spans,
            base,
            resolved.spacing_above,
            resolved.spacing_below,
        )
    }

    fn finish(self) -> LaidOutDocument {
        let total_height = self.cursor_y + self.style.vertical_padding;
        LaidOutDocument {
            packet: DocumentRenderPacket {
                viewport: self.viewport,
                content_bounds: Rect::from_xywh(0.0, 0.0, self.viewport.width, total_height),
                blocks: self.blocks,
                interactions: self.interactions,
            },
            fonts: self.fonts.into_table(),
        }
    }
}

#[cfg(test)]
mod tests;
