/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `WeldEngine` — the `inker::SurfaceEngine` impl + the host-supplied
//! `WeldProducerFactory` that builds a concrete CEF surface per spawn.

use std::sync::Arc;

use inker::{SurfaceEngine, SurfaceError, SurfaceProducer, SurfaceSpawnRequest};

use crate::producer::{WeldProducer, WeldSurface};

/// Engine ID this engine registers under.
///
/// Not yet in `inker::routing`'s constant set; add `ENGINE_WELD_CHROMIUM =
/// "weld.chromium"` there (kept out of the default policy, opt-in via pin /
/// override, like `ENGINE_SCRYING_WEB`) when wiring meerkat.
pub const WELD_CHROMIUM_ENGINE_ID: &str = "weld.chromium";

/// Host-supplied factory that builds a concrete [`WeldSurface`] for a resolved
/// [`SurfaceSpawnRequest`].
///
/// The engine can't build the surface itself: it needs an initialized
/// `welding::CefRuntime`, a `CefSurfaceProducer` bound to a host HWND/view, and
/// the host wgpu device for the import — none of which this crate depends on.
/// Crucially, the host must already have paid the **subprocess tax** (called
/// `CefRuntime::execute_process_from` at the top of `main()`); a factory built in
/// a process that skipped it will produce a blank OSR surface.
pub trait WeldProducerFactory: Send + Sync {
    /// Build a fresh CEF surface for this request, or a
    /// [`SurfaceError::SpawnFailed`] describing why.
    fn build(&self, request: &SurfaceSpawnRequest) -> Result<Box<dyn WeldSurface>, SurfaceError>;
}

/// `inker::SurfaceEngine` impl backed by wgpu-weld (CEF / Chromium).
pub struct WeldEngine {
    factory: Arc<dyn WeldProducerFactory>,
}

impl WeldEngine {
    /// Construct the engine. The host's `factory` knows how to build a concrete
    /// CEF surface.
    pub fn new(factory: Arc<dyn WeldProducerFactory>) -> Self {
        Self { factory }
    }
}

impl SurfaceEngine for WeldEngine {
    fn engine_id(&self) -> &str {
        WELD_CHROMIUM_ENGINE_ID
    }

    // a11y_capability defaults to Opaque: the CEF frame is an imported GPU
    // texture. CEF's CDP/DevTools could later back a bridge that upgrades this.

    #[tracing::instrument(level = "debug", skip(self, request), fields(url = %request.url))]
    fn spawn(
        &self,
        request: &SurfaceSpawnRequest,
    ) -> Result<Box<dyn SurfaceProducer>, SurfaceError> {
        let surface = self.factory.build(request).map_err(|err| {
            tracing::warn!(?err, "weld producer factory failed");
            err
        })?;
        Ok(Box::new(WeldProducer::new(surface)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inker::{
        CursorShape, EngineProfileBinding, FocusReason, KeyboardEvent, MouseEvent, NavigationEvent,
        PointerEvent, SurfaceEngineRegistry, SurfaceSettings, WebMessage,
        routing::{EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId},
    };

    use crate::producer::WeldFrame;

    /// Minimal surface stub: navigable, no frames, no events.
    struct StubSurface;

    impl WeldSurface for StubSurface {
        fn resize(&mut self, _: u32, _: u32) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn acquire_frame(&mut self) -> Result<Option<WeldFrame>, SurfaceError> {
            Ok(None)
        }
        fn load_url(&mut self, _: &str) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn load_html(&mut self, _: &str) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn reload(&mut self) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn go_back(&mut self) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn go_forward(&mut self) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn can_go_back(&self) -> bool {
            false
        }
        fn can_go_forward(&self) -> bool {
            false
        }
        fn notify_mouse(&mut self, _: MouseEvent) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn notify_pointer(&mut self, _: PointerEvent) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn notify_keyboard(&mut self, _: KeyboardEvent) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn focus(&mut self, _: FocusReason) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn poll_navigation_event(&mut self) -> Option<NavigationEvent> {
            None
        }
        fn poll_cursor_shape(&mut self) -> Option<CursorShape> {
            None
        }
        fn poll_web_message(&mut self) -> Option<WebMessage> {
            None
        }
        fn apply_settings(&mut self, _: &SurfaceSettings) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn capture_snapshot_png(&mut self) -> Result<Vec<u8>, SurfaceError> {
            Err(SurfaceError::Unsupported("stub".into()))
        }
    }

    struct StubFactory;
    impl WeldProducerFactory for StubFactory {
        fn build(&self, _: &SurfaceSpawnRequest) -> Result<Box<dyn WeldSurface>, SurfaceError> {
            Ok(Box::new(StubSurface))
        }
    }

    struct FailFactory;
    impl WeldProducerFactory for FailFactory {
        fn build(&self, _: &SurfaceSpawnRequest) -> Result<Box<dyn WeldSurface>, SurfaceError> {
            Err(SurfaceError::SpawnFailed("no host context".into()))
        }
    }

    fn stub_request() -> SurfaceSpawnRequest {
        SurfaceSpawnRequest {
            url: "https://example.com".into(),
            width: 800,
            height: 600,
            profile: EngineProfileBinding {
                user_data_dir: "/tmp/test-profile".into(),
            },
            fence_handle: None,
        }
    }

    fn decision() -> EngineRouteDecision {
        EngineRouteDecision {
            engine_id: WELD_CHROMIUM_ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("tile:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        }
    }

    #[test]
    fn registers_and_spawns_through_registry() {
        let mut reg = SurfaceEngineRegistry::new();
        reg.register(Box::new(WeldEngine::new(Arc::new(StubFactory))));
        assert!(reg.contains(WELD_CHROMIUM_ENGINE_ID));

        let mut producer = reg
            .spawn(&decision(), &stub_request())
            .ok()
            .expect("spawn ok");
        match producer.acquire_frame() {
            Ok(opt) => assert!(opt.is_none()),
            Err(err) => panic!("unexpected acquire_frame err: {err:?}"),
        }
    }

    #[test]
    fn factory_failure_surfaces_as_spawn_failed() {
        let engine = WeldEngine::new(Arc::new(FailFactory));
        let result = engine.spawn(&stub_request());
        assert!(matches!(result, Err(SurfaceError::SpawnFailed(_))));
    }
}
