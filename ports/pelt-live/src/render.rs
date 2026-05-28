/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ScriptedDom` → `netrender::Scene`.
//!
//! A focused mirror of [`pelt-viewer`'s `build_scene`](../../pelt-viewer/render.rs),
//! but pointed at a live, mutable [`ScriptedDom`] instead of parsing HTML: the
//! serval engine pipeline (cascade → layout → emit) runs over the same DOM that
//! [`ServalAppRunner`](xilem_serval::ServalAppRunner) diffs. This is the render
//! half of Stage 1b — state change → DOM diff → serval layout/paint, offline.
//!
//! `run_cascade` / `layout` / `emit_paint_list_with_layouts` are all generic
//! over `D: LayoutDom`, and `ScriptedDom` is a `LayoutDom`, so this compiles for
//! the scripted DOM exactly as it does for the static one. The image/fetcher
//! paths the static viewer carries are dropped: a counter has no `<img>` and no
//! `background-image`, so we lay out against an empty
//! [`ImagePlane`]/[`BackgroundImagePlane`].
//!
//! GPU-free by construction (no wgpu): scene *production* and presentation stay
//! separable, and the test driver asserts on the produced `Scene`/layout
//! without a window.

use paint_list_api::DeviceIntSize;
use serval_layout::{
    BackgroundImagePlane, FragmentPlane, ImagePlane, StylePlane, emit_paint_list_with_layouts,
    layout, run_cascade,
};
use serval_scripted_dom::{NodeId, ScriptedDom};

/// Run cascade → layout → paint-emit over `dom` and translate the paint list to
/// a [`netrender::Scene`] at `width`×`height`.
///
/// `stylesheets` are author CSS applied on top of serval's UA defaults. Unlike
/// the static viewer there is no inline `<style>` / `<link>` collection: the
/// chrome DOM the runner builds carries no document-embedded stylesheets, so the
/// caller's sheets are the whole author set.
pub fn scene_from_scripted_dom(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
) -> netrender::Scene {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
    );

    // A counter has no replaced content and no CSS backgrounds, so both image
    // planes are empty (the box tree's replaced-leaf sizing reads nothing).
    let images = ImagePlane::new();
    let bg_images = BackgroundImagePlane::new();

    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(dom, &styles, &images, viewport);
    let plist = emit_paint_list_with_layouts(
        dom,
        &styles,
        &fragments,
        &built,
        &text_ctx,
        &images,
        &bg_images,
        DeviceIntSize::new(width as i32, height as i32),
    );

    paint::translate_paint_list(&plist)
}

/// Run only the cascade → layout half (no paint emission) over `dom`, returning
/// the per-node [`FragmentPlane`].
///
/// The layout-level companion to [`scene_from_scripted_dom`]: it lets the test
/// driver assert that a node was reached by layout (`rect_of(node).is_some()`)
/// independent of paint emission, which is the plan's fallback assertion level.
pub fn fragments_from_scripted_dom(
    dom: &ScriptedDom,
    stylesheets: &[&str],
    width: u32,
    height: u32,
) -> FragmentPlane<NodeId> {
    let mut styles: StylePlane<NodeId> = StylePlane::new();
    run_cascade(
        dom,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
    );
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, _built, _text_ctx) = layout(dom, &styles, &images, viewport);
    fragments
}
