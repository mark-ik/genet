//! Retained Livery document ownership.

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use livery::media::Device;
use paint_list_api::DeviceIntSize;

use crate::{
    InteractionStates, LayoutError, LiveryPaintList, StyleSet, TextSystem,
    emit_paint_list_with_text_system, layout::layout_with_text_system, resolve_styles,
};

/// A static DOM plus the Livery state that should survive between frames.
///
/// Equal-size frames reuse the complete paint list. Resizes recascade media
/// queries, relayout, and repaint while retaining Parley's font database,
/// shaping scratch space, and shared font-resource allocations.
pub struct LiveryDocument<D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    dom: D,
    style_set: StyleSet,
    device: Device,
    interactions: InteractionStates<D::NodeId>,
    text: TextSystem,
    generation: u64,
    cached: Option<((u32, u32), LiveryPaintList)>,
}

impl<D> LiveryDocument<D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    pub fn new(dom: D, style_set: StyleSet, device: Device) -> Self {
        Self {
            dom,
            style_set,
            device,
            interactions: InteractionStates::default(),
            text: TextSystem::new(),
            generation: 0,
            cached: None,
        }
    }

    pub fn dom(&self) -> &D {
        &self.dom
    }

    pub fn text_system(&self) -> &TextSystem {
        &self.text
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn interactions_mut(&mut self) -> &mut InteractionStates<D::NodeId> {
        self.cached = None;
        &mut self.interactions
    }

    pub fn invalidate(&mut self) {
        self.cached = None;
    }

    pub fn frame(&mut self, width: u32, height: u32) -> Result<LiveryPaintList, LayoutError> {
        if let Some((viewport, list)) = &self.cached
            && *viewport == (width, height)
        {
            return Ok(list.clone());
        }

        self.device.viewport_width = width as f32;
        self.device.viewport_height = height as f32;
        let styles = resolve_styles(&self.dom, &self.style_set, &self.device, &self.interactions);
        let fragments = layout_with_text_system(
            &self.dom,
            &styles,
            width as f32,
            height as f32,
            &mut self.text,
        )?;
        self.generation = self.generation.saturating_add(1);
        let list = emit_paint_list_with_text_system(
            &self.dom,
            &styles,
            &fragments,
            DeviceIntSize::new(width as i32, height as i32),
            self.generation,
            &mut self.text,
        );
        self.cached = Some(((width, height), list.clone()));
        Ok(list)
    }

    pub fn into_dom(self) -> D {
        self.dom
    }
}
