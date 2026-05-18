/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pipeline-allocation probe for the `SendPaintList` path.
//!
//! Sibling to `paint_render_e2e.rs`: the latter validates the
//! full handle-messages → render → composite_texture path; this file
//! focuses on the dispatch-side plumbing — `SendPaintList` is
//! dispatched, pipeline state is allocated under the envelope's
//! `paint_info` pipeline_id, the renderer surfaces a master.

use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use netrender::{NetrenderOptions, boot, create_netrender_instance};
use paint::Paint;
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_list_api::{
    CommonPlacement, DeviceIntSize, EngineId, LayoutPoint, LayoutRect, PaintCmd, PaintEnvelope,
    PrimitiveFlags, RectItem,
};
use paint_types::{ColorF, PipelineId};
use servo_base::id::{PainterId, PipelineNamespace, PipelineNamespaceId, WebViewId};

fn ensure_pipeline_namespace() {
    PipelineNamespace::install(PipelineNamespaceId(1));
}

const VIEWPORT: u32 = 256;

fn synthesize_one_rect_paint_envelope() -> (PaintEnvelope, PaintDisplayListInfo, PipelineId) {
    let pid = PipelineId::default();
    let envelope = PaintEnvelope {
        engine: EngineId::SERVAL,
        viewport: DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        generation: 0,
        commands: vec![PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement {
                bounds: LayoutRect::new(
                    LayoutPoint::new(40.0, 40.0),
                    LayoutPoint::new(216.0, 216.0),
                ),
                flags: PrimitiveFlags::empty(),
            },
            color: ColorF {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0,
            },
        })],
    };

    let info = PaintDisplayListInfo::new(
        ViewportDetails {
            size: Size2D::new(VIEWPORT as f32, VIEWPORT as f32),
            hidpi_scale_factor: Scale::new(1.0),
        },
        paint_types::units::LayoutSize::new(VIEWPORT as f32, VIEWPORT as f32),
        pid,
        servo_base::Epoch(0),
        AxesScrollSensitivity {
            x: ScrollType::InputEvents | ScrollType::Script,
            y: ScrollType::InputEvents | ScrollType::Script,
        },
        true,
    );

    (envelope, info, pid)
}

#[test]
fn paint_list_render_e2e_drives_full_embedder_path() {
    let handles = boot().expect("wgpu boot");
    let renderer = create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size: Some(64),
            enable_vello: true,
            ..Default::default()
        },
    )
    .expect("create_netrender_instance");

    let paint_rc = Paint::new_for_test();
    let paint = paint_rc.borrow();

    ensure_pipeline_namespace();
    let painter_id = PainterId::next();
    paint.install_renderer(painter_id, renderer);
    let webview_id = WebViewId::new(painter_id);

    let (envelope, paint_info, _pid) = synthesize_one_rect_paint_envelope();
    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope,
        paint_info,
    }]);

    paint.render(webview_id);

    let master = paint
        .composite_texture(painter_id)
        .expect("composite_texture should return the master after render");
    let size = master.size();
    assert_eq!(size.width, VIEWPORT, "master texture width");
    assert_eq!(size.height, VIEWPORT, "master texture height");
    assert_eq!(
        master.format(),
        wgpu::TextureFormat::Rgba8Unorm,
        "master texture format should match Paint::render's request"
    );
}

#[test]
fn paint_list_send_paint_list_allocates_pipeline_state() {
    let paint_rc = Paint::new_for_test();
    let paint = paint_rc.borrow();

    ensure_pipeline_namespace();
    let painter_id = PainterId::next();
    let webview_id = WebViewId::new(painter_id);

    let (envelope, paint_info, pid) = synthesize_one_rect_paint_envelope();
    // No renderer installed — this exercises the SendPaintList arm
    // in isolation. The pipeline state should still be allocated;
    // only the eventual `render(webview_id)` becomes a no-op.
    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope,
        paint_info,
    }]);

    // The translated Scene should be retrievable by pipeline_id.
    assert!(
        paint.pipeline_scene(pid).is_some(),
        "SendPaintList arm should allocate pipeline state for the envelope's paint_info.pipeline_id"
    );
}
