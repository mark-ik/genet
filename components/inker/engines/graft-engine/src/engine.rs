/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `GraftEngine` — the `inker::SurfaceEngine` impl + the host-supplied
//! `GraftProducerFactory` that builds a concrete graft surface per spawn.

use std::sync::Arc;

use inker::{SurfaceEngine, SurfaceError, SurfaceProducer, SurfaceSpawnRequest};

use crate::producer::{GraftProducer, GraftSurface};

/// Engine ID this engine registers under.
///
/// Not yet in `inker::routing`'s constant set; add `ENGINE_GRAFT_SERVO =
/// "graft.servo"` there (kept out of the default policy, opt-in via pin /
/// override, exactly like `ENGINE_SCRYING_WEB`) when wiring meerkat. The
/// `engine_id_matches_routing_constant` assertion in `scrying-engine` is the
/// shape to mirror once the constant exists.
pub const GRAFT_SERVO_ENGINE_ID: &str = "graft.servo";

/// Host-supplied factory that builds a concrete [`GraftSurface`] for a resolved
/// [`SurfaceSpawnRequest`].
///
/// The engine can't build the surface itself: it needs the host's wgpu device,
/// an embedded `servo::Servo` instance + `WebView`, and the
/// `ServoWgpuInteropAdapter` bound to that device — none of which this crate
/// depends on. The host implements this trait once (behind its `engine-graft`
/// feature) and hands an `Arc` of it to [`GraftEngine::new`].
pub trait GraftProducerFactory: Send + Sync {
    /// Build a fresh graft surface for this request, or a
    /// [`SurfaceError::SpawnFailed`] describing why.
    fn build(&self, request: &SurfaceSpawnRequest) -> Result<Box<dyn GraftSurface>, SurfaceError>;
}

/// `inker::SurfaceEngine` impl backed by wgpu-graft.
pub struct GraftEngine {
    factory: Arc<dyn GraftProducerFactory>,
}

impl GraftEngine {
    /// Construct the engine. The host's `factory` knows how to build a concrete
    /// graft surface.
    pub fn new(factory: Arc<dyn GraftProducerFactory>) -> Self {
        Self { factory }
    }
}

impl SurfaceEngine for GraftEngine {
    fn engine_id(&self) -> &str {
        GRAFT_SERVO_ENGINE_ID
    }

    // a11y_capability defaults to Opaque: the graft frame is an imported GPU
    // texture with no semantics the host can read. A future Servo->AccessKit
    // bridge surfaced through GraftSurface could upgrade this to Partial.

    #[tracing::instrument(level = "debug", skip(self, request), fields(url = %request.url))]
    fn spawn(
        &self,
        request: &SurfaceSpawnRequest,
    ) -> Result<Box<dyn SurfaceProducer>, SurfaceError> {
        let surface = self.factory.build(request).map_err(|err| {
            tracing::warn!(?err, "graft producer factory failed");
            err
        })?;
        Ok(Box::new(GraftProducer::new(surface)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inker::{
        EngineProfileBinding, SurfaceEngineRegistry,
        routing::{EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId},
    };

    use crate::producer::GraftFrame;
    use inker::{
        CursorShape, FocusReason, KeyboardEvent, MouseEvent, NavigationEvent, PointerEvent,
        SurfaceSettings, WebMessage,
    };

    /// Minimal surface stub: navigable, no frames, no events. Drives the spawn
    /// pipeline through the registry without a Servo instance.
    struct StubSurface;

    impl GraftSurface for StubSurface {
        fn resize(&mut self, _: u32, _: u32) -> Result<(), SurfaceError> {
            Ok(())
        }
        fn acquire_frame(&mut self) -> Result<Option<GraftFrame>, SurfaceError> {
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
    impl GraftProducerFactory for StubFactory {
        fn build(&self, _: &SurfaceSpawnRequest) -> Result<Box<dyn GraftSurface>, SurfaceError> {
            Ok(Box::new(StubSurface))
        }
    }

    struct FailFactory;
    impl GraftProducerFactory for FailFactory {
        fn build(&self, _: &SurfaceSpawnRequest) -> Result<Box<dyn GraftSurface>, SurfaceError> {
            Err(SurfaceError::SpawnFailed("no host context".into()))
        }
    }

    fn stub_request() -> SurfaceSpawnRequest {
        SurfaceSpawnRequest {
            url: "https://servo.org".into(),
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
            engine_id: GRAFT_SERVO_ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("tile:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        }
    }

    #[test]
    fn registers_and_spawns_through_registry() {
        let mut reg = SurfaceEngineRegistry::new();
        reg.register(Box::new(GraftEngine::new(Arc::new(StubFactory))));
        assert!(reg.contains(GRAFT_SERVO_ENGINE_ID));

        let mut producer = reg
            .spawn(&decision(), &stub_request())
            .ok()
            .expect("spawn ok");
        // StubSurface yields no frame.
        match producer.acquire_frame() {
            Ok(opt) => assert!(opt.is_none()),
            Err(err) => panic!("unexpected acquire_frame err: {err:?}"),
        }
    }

    #[test]
    fn factory_failure_surfaces_as_spawn_failed() {
        let engine = GraftEngine::new(Arc::new(FailFactory));
        let result = engine.spawn(&stub_request());
        assert!(matches!(result, Err(SurfaceError::SpawnFailed(_))));
    }
}
