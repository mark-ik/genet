//! Retained Livery document ownership.

use std::{collections::HashMap, hash::Hash};

use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
use livery::PropertyId;
use livery::cascade::DeclaredValue;
use livery::media::Device;
use livery::{
    PropertyValue,
    selector::StatePseudoClass,
    stylesheet::Keyframes,
    values::{AnimationName, Color, Opacity, Overflow, TimingFunction, TransitionProperty},
};
use paint_list_api::DeviceIntSize;

use crate::{
    FragmentPlane, InteractionStates, LayoutError, LiveryPaintList, StylePlane, StyleSet,
    TextSystem, emit_paint_list_with_text_system_scrolled_with_images, hit_test_with_scroll,
    layout::layout_with_text_system, resolve_styles,
};

/// What a Livery click resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClickOutcome {
    None,
    Focused,
    Scrolled,
    Navigate(String),
}

/// A link rectangle retained from the last layout pass.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkTarget {
    pub url: String,
    pub rect: [f32; 4],
}

struct LayoutState<Id> {
    viewport: (u32, u32),
    styles: StylePlane<Id>,
    fragments: FragmentPlane<Id>,
    content_width: f32,
    content_height: f32,
}

#[derive(Clone, Copy)]
struct OpacityTransition<Id> {
    node: Id,
    from: f32,
    to: f32,
    start_ms: f64,
    duration_ms: f64,
    automatic: bool,
}

#[derive(Clone, Copy)]
struct BackgroundColorTransition<Id> {
    node: Id,
    from: Color,
    to: Color,
    start_ms: f64,
    duration_ms: f64,
    automatic: bool,
}

#[derive(Clone)]
struct KeyframeAnimation<Id> {
    node: Id,
    name: Box<str>,
    start_ms: f64,
    duration_ms: f64,
    timing: TimingFunction,
}

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
    layout: Option<LayoutState<D::NodeId>>,
    viewport: (u32, u32),
    scroll: (f32, f32),
    focused_chain: Vec<D::NodeId>,
    clock_ms: f64,
    opacity_transition: Option<OpacityTransition<D::NodeId>>,
    background_color_transition: Option<BackgroundColorTransition<D::NodeId>>,
    keyframe_animation: Option<KeyframeAnimation<D::NodeId>>,
    nested_scroll: HashMap<D::NodeId, (f32, f32)>,
    image_sources: HashMap<String, Vec<u8>>,
}

impl<D> LiveryDocument<D>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    pub fn new(dom: D, style_set: StyleSet, device: Device) -> Self {
        let viewport = (
            device.viewport_width.max(0.0) as u32,
            device.viewport_height.max(0.0) as u32,
        );
        Self {
            dom,
            style_set,
            device,
            interactions: InteractionStates::default(),
            text: TextSystem::new(),
            generation: 0,
            cached: None,
            layout: None,
            viewport,
            scroll: (0.0, 0.0),
            focused_chain: Vec::new(),
            clock_ms: 0.0,
            opacity_transition: None,
            background_color_transition: None,
            keyframe_animation: None,
            nested_scroll: HashMap::new(),
            image_sources: HashMap::new(),
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
        self.layout = None;
    }

    /// Supply host-resolved image bytes for a non-data URL. The CSS engine
    /// still owns decoding and paint-key allocation; the host owns URL
    /// resolution and fetching.
    pub fn set_image_resource(&mut self, url: impl Into<String>, bytes: Vec<u8>) {
        self.image_sources.insert(url.into(), bytes);
        self.cached = None;
    }

    pub fn frame(&mut self, width: u32, height: u32) -> Result<LiveryPaintList, LayoutError> {
        if let Some((viewport, list)) = &self.cached
            && *viewport == (width, height)
        {
            return Ok(list.clone().translated(-self.scroll.0, -self.scroll.1));
        }

        self.viewport = (width, height);
        self.device.viewport_width = width as f32;
        self.device.viewport_height = height as f32;
        self.finish_completed_opacity_transition();
        self.finish_completed_background_color_transition();
        let mut styles =
            resolve_styles(&self.dom, &self.style_set, &self.device, &self.interactions);
        self.schedule_opacity_transition(&styles);
        self.schedule_background_color_transition(&styles);
        self.schedule_keyframe_animation(&styles);
        self.apply_opacity_transition(&mut styles);
        self.apply_background_color_transition(&mut styles);
        self.apply_keyframe_animation(&mut styles);
        let fragments = layout_with_text_system(
            &self.dom,
            &styles,
            width as f32,
            height as f32,
            &mut self.text,
            &self.image_sources,
        )?;
        let (content_width, content_height) = self.document_content_extent(&styles, &fragments);
        self.layout = Some(LayoutState {
            viewport: (width, height),
            styles: styles.clone(),
            fragments: fragments.clone(),
            content_width,
            content_height,
        });
        self.clamp_scroll();
        self.clamp_nested_scroll();
        self.generation = self.generation.saturating_add(1);
        let list = emit_paint_list_with_text_system_scrolled_with_images(
            &self.dom,
            &styles,
            &fragments,
            DeviceIntSize::new(width as i32, height as i32),
            self.generation,
            &mut self.text,
            &self.nested_scroll,
            &self.image_sources,
        );
        self.cached = Some(((width, height), list.clone()));
        Ok(list.translated(-self.scroll.0, -self.scroll.1))
    }

    /// Return the current viewport scroll offset.
    pub fn scroll(&self) -> (f32, f32) {
        self.scroll
    }

    /// Start a host-driven opacity transition for one retained element. This
    /// is the runtime clock seam. CSS transitions use the same clock when the bounded transition
    /// longhands are present; this explicit method remains useful to hosts
    /// that need a direct paint-only animation.
    pub fn animate_opacity(
        &mut self,
        node: D::NodeId,
        from: f32,
        to: f32,
        start_ms: f64,
        duration_ms: f64,
    ) -> bool {
        if !from.is_finite()
            || !to.is_finite()
            || !start_ms.is_finite()
            || !duration_ms.is_finite()
            || duration_ms < 0.0
        {
            return false;
        }
        self.clock_ms = start_ms;
        self.opacity_transition = Some(OpacityTransition {
            node,
            from: from.clamp(0.0, 1.0),
            to: to.clamp(0.0, 1.0),
            start_ms,
            duration_ms,
            automatic: false,
        });
        self.cached = None;
        true
    }

    /// Advance retained animation time. A following frame samples the
    /// interpolated value without re-running layout.
    pub fn pump(&mut self, now_ms: f64) -> bool {
        if (self.opacity_transition.is_none()
            && self.background_color_transition.is_none()
            && self.keyframe_animation.is_none())
            || !now_ms.is_finite()
        {
            return false;
        }
        let next = now_ms.max(self.clock_ms);
        let changed = next != self.clock_ms;
        self.clock_ms = next;
        if changed {
            self.cached = None;
        }
        changed
    }

    pub fn settled(&self) -> bool {
        let opacity_settled = self
            .opacity_transition
            .is_none_or(|transition| self.clock_ms >= transition.start_ms + transition.duration_ms);
        let keyframe_settled = self
            .keyframe_animation
            .as_ref()
            .is_none_or(|animation| self.clock_ms >= animation.start_ms + animation.duration_ms);
        let background_color_settled = self
            .background_color_transition
            .is_none_or(|transition| self.clock_ms >= transition.start_ms + transition.duration_ms);
        opacity_settled && background_color_settled && keyframe_settled
    }

    /// Scroll the document viewport by device pixels. Wheel deltas that need
    /// position-aware nested routing go through [`Self::scroll_at`].
    pub fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        let before = self.scroll;
        self.scroll.0 += dx;
        self.scroll.1 += dy;
        self.clamp_scroll();
        before != self.scroll
    }

    pub fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let Some(layout) = self.layout.as_ref() else {
            return false;
        };
        let mut node = hit_test_with_scroll(
            &self.dom,
            &layout.styles,
            &layout.fragments,
            &self.nested_scroll,
            x + self.scroll.0,
            y + self.scroll.1,
        );
        while let Some(candidate) = node {
            if let Some(next) = self.scroll_step(layout, candidate, dx, dy) {
                self.nested_scroll.insert(candidate, next);
                self.cached = None;
                return true;
            }
            node = self.dom.parent(candidate);
        }
        self.scroll_by(dx, dy)
    }

    pub fn scroll_to(&mut self, y: f32) {
        self.scroll.1 = y;
        self.clamp_scroll();
    }

    pub fn scroll_line(&mut self, direction: i8) -> bool {
        self.scroll_by(0.0, 40.0 * f32::from(direction))
    }

    pub fn scroll_page(&mut self, direction: i8) -> bool {
        let amount = self.viewport.1 as f32 * 0.9;
        self.scroll_by(0.0, amount * f32::from(direction))
    }

    pub fn content_height(&self, fallback: u32) -> u32 {
        self.layout
            .as_ref()
            .map_or(fallback, |layout| layout.content_height.ceil() as u32)
    }

    /// Retained per-element scroll offsets for hosts that draw their own
    /// scrollbar or accessibility overlay.
    pub fn element_scroll(&self) -> &HashMap<D::NodeId, (f32, f32)> {
        &self.nested_scroll
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<D::NodeId> {
        let layout = self.layout.as_ref()?;
        hit_test_with_scroll(
            &self.dom,
            &layout.styles,
            &layout.fragments,
            &self.nested_scroll,
            x + self.scroll.0,
            y + self.scroll.1,
        )
    }

    pub fn links(&self) -> Vec<LinkTarget> {
        let Some(layout) = self.layout.as_ref() else {
            return Vec::new();
        };
        let mut links = Vec::new();
        self.collect_links(self.dom.document(), layout, &mut links);
        links
    }

    pub fn click_at(&mut self, x: f32, y: f32) -> ClickOutcome {
        let Some(target) = self.hit_test(x, y) else {
            return ClickOutcome::None;
        };
        let focus_target = self.focusable_ancestor(target);
        let focused = focus_target.is_some_and(|id| self.focus(id));
        let href = self.link_ancestor(target);
        if let Some(href) = href {
            if let Some(fragment) = href
                .strip_prefix('#')
                .filter(|fragment| !fragment.is_empty())
                && self.scroll_to_fragment(fragment)
            {
                return ClickOutcome::Scrolled;
            }
            return ClickOutcome::Navigate(href);
        }
        if focused {
            ClickOutcome::Focused
        } else {
            ClickOutcome::None
        }
    }

    fn clamp_scroll(&mut self) {
        let Some(layout) = self.layout.as_ref() else {
            self.scroll = (0.0, 0.0);
            return;
        };
        let (scroll_x, scroll_y) = self.scrollable_axes(layout);
        let max_x = if scroll_x {
            (layout.content_width - layout.viewport.0 as f32).max(0.0)
        } else {
            0.0
        };
        let max_y = if scroll_y {
            (layout.content_height - layout.viewport.1 as f32).max(0.0)
        } else {
            0.0
        };
        self.scroll.0 = self.scroll.0.clamp(0.0, max_x);
        self.scroll.1 = self.scroll.1.clamp(0.0, max_y);
    }

    fn document_content_extent(
        &self,
        styles: &StylePlane<D::NodeId>,
        fragments: &FragmentPlane<D::NodeId>,
    ) -> (f32, f32) {
        let mut extent = (0.0, 0.0);
        for child in self.dom.dom_children(self.dom.document()) {
            self.extend_content_extent(child, styles, fragments, &mut extent, false);
        }
        extent
    }

    fn extend_content_extent(
        &self,
        id: D::NodeId,
        styles: &StylePlane<D::NodeId>,
        fragments: &FragmentPlane<D::NodeId>,
        extent: &mut (f32, f32),
        nested: bool,
    ) {
        let Some(style) = styles.get(id) else {
            return;
        };
        if style.display == livery::values::Display::None {
            return;
        }
        if let Some(fragment) = fragments.get(id) {
            extent.0 = extent.0.max(fragment.x + fragment.width);
            extent.1 = extent.1.max(fragment.y + fragment.height);
        }
        if nested && self.clips_content(style) {
            return;
        }
        for child in self.dom.dom_children(id) {
            self.extend_content_extent(child, styles, fragments, extent, true);
        }
    }

    fn clamp_nested_scroll(&mut self) {
        let Some(layout) = self.layout.as_ref() else {
            self.nested_scroll.clear();
            return;
        };
        let keys = self.nested_scroll.keys().copied().collect::<Vec<_>>();
        for node in keys {
            let Some(style) = layout.styles.get(node) else {
                self.nested_scroll.remove(&node);
                continue;
            };
            if !self.is_scroll_container(style) {
                self.nested_scroll.remove(&node);
                continue;
            }
            let (max_x, max_y) = self.scroll_extent(layout, node);
            if let Some(offset) = self.nested_scroll.get_mut(&node) {
                offset.0 = offset.0.clamp(0.0, max_x);
                offset.1 = offset.1.clamp(0.0, max_y);
            }
        }
    }

    fn scroll_step(
        &self,
        layout: &LayoutState<D::NodeId>,
        node: D::NodeId,
        dx: f32,
        dy: f32,
    ) -> Option<(f32, f32)> {
        let style = layout.styles.get(node)?;
        if !self.is_scroll_container(style) {
            return None;
        }
        let (max_x, max_y) = self.scroll_extent(layout, node);
        let current = self.nested_scroll.get(&node).copied().unwrap_or((0.0, 0.0));
        let next = (
            if self.scrolls_x(style) {
                (current.0 + dx).clamp(0.0, max_x)
            } else {
                current.0
            },
            if self.scrolls_y(style) {
                (current.1 + dy).clamp(0.0, max_y)
            } else {
                current.1
            },
        );
        if next == current { None } else { Some(next) }
    }

    fn scroll_extent(&self, layout: &LayoutState<D::NodeId>, node: D::NodeId) -> (f32, f32) {
        let Some(container) = layout.fragments.get(node) else {
            return (0.0, 0.0);
        };
        let mut extent = (0.0, 0.0);
        for child in self.dom.dom_children(node) {
            self.extend_nested_extent(child, node, layout, &mut extent);
        }
        (
            (extent.0 - container.width).max(0.0),
            (extent.1 - container.height).max(0.0),
        )
    }

    fn extend_nested_extent(
        &self,
        id: D::NodeId,
        container: D::NodeId,
        layout: &LayoutState<D::NodeId>,
        extent: &mut (f32, f32),
    ) {
        let Some(style) = layout.styles.get(id) else {
            return;
        };
        if style.display == livery::values::Display::None {
            return;
        }
        if let (Some(container), Some(fragment)) =
            (layout.fragments.get(container), layout.fragments.get(id))
        {
            extent.0 = extent.0.max(fragment.x + fragment.width - container.x);
            extent.1 = extent.1.max(fragment.y + fragment.height - container.y);
        }
        if self.clips_content(style) {
            return;
        }
        for child in self.dom.dom_children(id) {
            self.extend_nested_extent(child, container, layout, extent);
        }
    }

    fn is_scroll_container(&self, style: &livery::ComputedValues) -> bool {
        self.scrolls_x(style) || self.scrolls_y(style)
    }

    fn clips_content(&self, style: &livery::ComputedValues) -> bool {
        style.overflow_x != Overflow::Visible || style.overflow_y != Overflow::Visible
    }

    fn scrolls_x(&self, style: &livery::ComputedValues) -> bool {
        matches!(style.overflow_x, Overflow::Auto | Overflow::Scroll)
    }

    fn scrolls_y(&self, style: &livery::ComputedValues) -> bool {
        matches!(style.overflow_y, Overflow::Auto | Overflow::Scroll)
    }

    fn apply_opacity_transition(&self, styles: &mut StylePlane<D::NodeId>) {
        let Some(transition) = self.opacity_transition else {
            return;
        };
        let progress = if transition.duration_ms == 0.0 {
            1.0
        } else {
            ((self.clock_ms - transition.start_ms) / transition.duration_ms).clamp(0.0, 1.0) as f32
        };
        let value = transition.from + (transition.to - transition.from) * progress;
        if let Some(style) = styles.get_mut(transition.node) {
            style.opacity = Opacity::from_value(value);
        }
    }

    fn apply_background_color_transition(&self, styles: &mut StylePlane<D::NodeId>) {
        let Some(transition) = self.background_color_transition else {
            return;
        };
        let progress = if transition.duration_ms == 0.0 {
            1.0
        } else {
            ((self.clock_ms - transition.start_ms) / transition.duration_ms).clamp(0.0, 1.0) as f32
        };
        let value = transition.from.interpolate(transition.to, progress);
        if let Some(style) = styles.get_mut(transition.node) {
            style.background_color = value;
        }
    }

    fn apply_keyframe_animation(&self, styles: &mut StylePlane<D::NodeId>) {
        let Some(animation) = self.keyframe_animation.as_ref() else {
            return;
        };
        let Some(keyframes) = self.style_set.keyframes(&animation.name) else {
            return;
        };
        let progress = if animation.duration_ms == 0.0 {
            1.0
        } else {
            ((self.clock_ms - animation.start_ms) / animation.duration_ms).clamp(0.0, 1.0) as f32
        };
        let progress = animation.timing.sample(progress);
        let base = styles
            .get(animation.node)
            .map_or(1.0, |style| style.opacity.value());
        if let Some(value) = keyframe_opacity(keyframes, progress, base)
            && let Some(style) = styles.get_mut(animation.node)
        {
            style.opacity = Opacity::from_value(value);
        }
    }

    fn schedule_keyframe_animation(&mut self, styles: &StylePlane<D::NodeId>) {
        let candidate = self.find_keyframe_animation(self.dom.document(), styles);
        let Some((node, name, duration_ms, timing)) = candidate else {
            self.keyframe_animation = None;
            return;
        };
        if self.keyframe_animation.as_ref().is_some_and(|animation| {
            animation.node == node
                && animation.name.as_ref() == name.as_str()
                && animation.duration_ms == duration_ms
                && animation.timing == timing
        }) {
            return;
        }
        self.keyframe_animation = Some(KeyframeAnimation {
            node,
            name: name.into_boxed_str(),
            start_ms: self.clock_ms,
            duration_ms,
            timing,
        });
    }

    fn find_keyframe_animation(
        &self,
        id: D::NodeId,
        styles: &StylePlane<D::NodeId>,
    ) -> Option<(D::NodeId, String, f64, TimingFunction)> {
        if let Some(style) = styles.get(id)
            && let AnimationName::Name(name) = &style.animation_name
        {
            let duration_ms = f64::from(style.animation_duration.milliseconds());
            if duration_ms > 0.0 && self.style_set.keyframes(name).is_some() {
                return Some((
                    id,
                    name.to_string(),
                    duration_ms,
                    style.animation_timing_function,
                ));
            }
        }
        self.dom
            .dom_children(id)
            .find_map(|child| self.find_keyframe_animation(child, styles))
    }

    fn finish_completed_opacity_transition(&mut self) {
        let Some(transition) = self.opacity_transition else {
            return;
        };
        if !transition.automatic || self.clock_ms < transition.start_ms + transition.duration_ms {
            return;
        }
        if let Some(layout) = self.layout.as_mut()
            && let Some(style) = layout.styles.get_mut(transition.node)
        {
            style.opacity = Opacity::from_value(transition.to);
        }
        self.opacity_transition = None;
    }

    fn finish_completed_background_color_transition(&mut self) {
        let Some(transition) = self.background_color_transition else {
            return;
        };
        if !transition.automatic || self.clock_ms < transition.start_ms + transition.duration_ms {
            return;
        }
        if let Some(layout) = self.layout.as_mut()
            && let Some(style) = layout.styles.get_mut(transition.node)
        {
            style.background_color = transition.to;
        }
        self.background_color_transition = None;
    }

    fn schedule_opacity_transition(&mut self, styles: &StylePlane<D::NodeId>) {
        if self.opacity_transition.is_some() {
            return;
        }
        let Some(previous) = self.layout.as_ref().map(|layout| &layout.styles) else {
            return;
        };
        let Some((node, from, to, duration_ms)) =
            self.find_opacity_transition(self.dom.document(), previous, styles)
        else {
            return;
        };
        self.opacity_transition = Some(OpacityTransition {
            node,
            from,
            to,
            start_ms: self.clock_ms,
            duration_ms,
            automatic: true,
        });
    }

    fn find_opacity_transition(
        &self,
        id: D::NodeId,
        previous: &StylePlane<D::NodeId>,
        styles: &StylePlane<D::NodeId>,
    ) -> Option<(D::NodeId, f32, f32, f64)> {
        if let (Some(old), Some(new)) = (previous.get(id), styles.get(id)) {
            let duration_ms = f64::from(new.transition_duration.milliseconds());
            let accepts_opacity = matches!(
                new.transition_property,
                TransitionProperty::All
                    | TransitionProperty::Opacity
                    | TransitionProperty::OpacityAndBackgroundColor
            );
            if accepts_opacity && duration_ms > 0.0 && old.opacity.value() != new.opacity.value() {
                return Some((id, old.opacity.value(), new.opacity.value(), duration_ms));
            }
        }
        self.dom
            .dom_children(id)
            .find_map(|child| self.find_opacity_transition(child, previous, styles))
    }

    fn schedule_background_color_transition(&mut self, styles: &StylePlane<D::NodeId>) {
        if self.background_color_transition.is_some() {
            return;
        }
        let Some(previous) = self.layout.as_ref().map(|layout| &layout.styles) else {
            return;
        };
        let Some((node, from, to, duration_ms)) =
            self.find_background_color_transition(self.dom.document(), previous, styles)
        else {
            return;
        };
        self.background_color_transition = Some(BackgroundColorTransition {
            node,
            from,
            to,
            start_ms: self.clock_ms,
            duration_ms,
            automatic: true,
        });
    }

    fn find_background_color_transition(
        &self,
        id: D::NodeId,
        previous: &StylePlane<D::NodeId>,
        styles: &StylePlane<D::NodeId>,
    ) -> Option<(D::NodeId, Color, Color, f64)> {
        if let (Some(old), Some(new)) = (previous.get(id), styles.get(id)) {
            let duration_ms = f64::from(new.transition_duration.milliseconds());
            let accepts_background_color = matches!(
                new.transition_property,
                TransitionProperty::All
                    | TransitionProperty::BackgroundColor
                    | TransitionProperty::OpacityAndBackgroundColor
            );
            if accepts_background_color
                && duration_ms > 0.0
                && old.background_color != new.background_color
            {
                return Some((id, old.background_color, new.background_color, duration_ms));
            }
        }
        self.dom
            .dom_children(id)
            .find_map(|child| self.find_background_color_transition(child, previous, styles))
    }

    fn scrollable_axes(&self, layout: &LayoutState<D::NodeId>) -> (bool, bool) {
        let root = self
            .dom
            .dom_children(self.dom.document())
            .find(|id| self.dom.kind(*id) == NodeKind::Element);
        let Some(root) = root else {
            return (true, true);
        };
        let Some(style) = layout.styles.get(root) else {
            return (true, true);
        };
        (
            !matches!(style.overflow_x, Overflow::Hidden | Overflow::Clip),
            !matches!(style.overflow_y, Overflow::Hidden | Overflow::Clip),
        )
    }

    fn focus(&mut self, id: D::NodeId) -> bool {
        for old in self.focused_chain.drain(..) {
            self.interactions.set(old, StatePseudoClass::Focus, false);
            self.interactions
                .set(old, StatePseudoClass::FocusWithin, false);
        }
        self.interactions.set(id, StatePseudoClass::Focus, true);
        let mut chain = vec![id];
        let mut parent = self.dom.parent(id);
        while let Some(ancestor) = parent {
            if self.dom.kind(ancestor) == NodeKind::Element {
                self.interactions
                    .set(ancestor, StatePseudoClass::FocusWithin, true);
                chain.push(ancestor);
            }
            parent = self.dom.parent(ancestor);
        }
        self.focused_chain = chain;
        self.cached = None;
        true
    }

    fn focusable_ancestor(&self, mut id: D::NodeId) -> Option<D::NodeId> {
        loop {
            if self.is_focusable(id) {
                return Some(id);
            }
            id = self.dom.parent(id)?;
        }
    }

    fn is_focusable(&self, id: D::NodeId) -> bool {
        if self.dom.kind(id) != NodeKind::Element {
            return false;
        }
        let Some(name) = self.dom.element_name(id) else {
            return false;
        };
        let local = name.local.as_ref();
        local.eq_ignore_ascii_case("a") && self.attribute(id, "href").is_some()
            || matches!(
                local.to_ascii_lowercase().as_str(),
                "button" | "input" | "select" | "textarea"
            )
            || self.attribute(id, "tabindex").is_some()
    }

    fn link_ancestor(&self, mut id: D::NodeId) -> Option<String> {
        loop {
            if self.dom.kind(id) == NodeKind::Element
                && self
                    .dom
                    .element_name(id)
                    .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("a"))
                && let Some(href) = self.attribute(id, "href")
            {
                return Some(href.to_owned());
            }
            id = self.dom.parent(id)?;
        }
    }

    fn scroll_to_fragment(&mut self, fragment: &str) -> bool {
        let Some(target) = find_id(&self.dom, self.dom.document(), fragment) else {
            return false;
        };
        let Some(y) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.fragments.get(target).map(|fragment| fragment.y))
        else {
            return false;
        };
        self.scroll_to(y);
        true
    }

    fn collect_links(
        &self,
        id: D::NodeId,
        layout: &LayoutState<D::NodeId>,
        links: &mut Vec<LinkTarget>,
    ) {
        if self.dom.kind(id) == NodeKind::Element
            && let Some(href) = self.attribute(id, "href")
            && let Some(fragment) = layout.fragments.get(id)
            && let Some(style) = layout.styles.get(id)
            && style.display != livery::values::Display::None
            && style.visibility == livery::values::Visibility::Visible
            && style.pointer_events == livery::values::PointerEvents::Auto
        {
            let (nested_x, nested_y) = self.ancestor_scroll(id);
            links.push(LinkTarget {
                url: href.to_owned(),
                rect: [
                    fragment.x - self.scroll.0 - nested_x,
                    fragment.y - self.scroll.1 - nested_y,
                    fragment.width,
                    fragment.height,
                ],
            });
        }
        for child in self.dom.dom_children(id) {
            self.collect_links(child, layout, links);
        }
    }

    fn ancestor_scroll(&self, id: D::NodeId) -> (f32, f32) {
        let mut offset = (0.0, 0.0);
        let mut parent = self.dom.parent(id);
        while let Some(ancestor) = parent {
            if let Some(scroll) = self.nested_scroll.get(&ancestor) {
                offset.0 += scroll.0;
                offset.1 += scroll.1;
            }
            parent = self.dom.parent(ancestor);
        }
        offset
    }

    fn attribute(&self, id: D::NodeId, local: &str) -> Option<&str> {
        self.dom
            .attribute(id, &Namespace::from(""), &LocalName::from(local))
    }

    pub fn into_dom(self) -> D {
        self.dom
    }
}

fn keyframe_opacity(keyframes: &Keyframes, progress: f32, fallback: f32) -> Option<f32> {
    let samples = keyframes
        .frames()
        .iter()
        .filter_map(|frame| {
            frame
                .declarations()
                .declarations
                .iter()
                .find(|declaration| declaration.property == PropertyId::Opacity)
                .and_then(|declaration| match &declaration.value {
                    DeclaredValue::Value(PropertyValue::Opacity(value)) => {
                        Some((frame.offset(), value.value()))
                    },
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    let Some(&(first_offset, first_value)) = samples.first() else {
        return Some(fallback);
    };
    if progress <= first_offset {
        return Some(first_value);
    }
    for pair in samples.windows(2) {
        let [(left_offset, left_value), (right_offset, right_value)] = pair else {
            continue;
        };
        if progress <= *right_offset {
            let span = (*right_offset - *left_offset).max(f32::EPSILON);
            let local = ((progress - *left_offset) / span).clamp(0.0, 1.0);
            return Some(*left_value + (*right_value - *left_value) * local);
        }
    }
    samples.last().map(|(_, value)| *value)
}

fn find_id<D: LayoutDom>(dom: &D, id: D::NodeId, target: &str) -> Option<D::NodeId> {
    if dom.kind(id) == NodeKind::Element
        && dom
            .attribute(id, &Namespace::from(""), &LocalName::from("id"))
            .is_some_and(|value| value == target)
    {
        return Some(id);
    }
    dom.dom_children(id)
        .find_map(|child| find_id(dom, child, target))
}
