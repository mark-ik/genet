/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Scripting-profile dispatch for the Constellation.
//!
//! The Constellation is generic over a `ScriptThreadFactory` and a
//! `ServiceWorkerManagerFactory`. The browser composition uses
//! `script::ScriptThread` and `script::ServiceWorkerManager`. The script-free
//! composition (Pelt's viewer / static profiles) uses the no-op factories
//! defined here, which satisfy the trait surface without spawning a real
//! script thread or service-worker manager.
//!
//! The selection is made at `Servo::new` time via [`ScriptingProfile`] and
//! routed through the call to `Constellation::<STF, SWF>::start(...)`. Each
//! match arm monomorphizes to a different concrete `Constellation` type.

use std::sync::Arc;
use std::thread::{self, JoinHandle};

use background_hang_monitor_api::BackgroundHangMonitorRegister;
use layout_api::{LayoutFactory, ScriptThreadFactory};
use net_traits::image_cache::ImageCacheFactory;
use script_traits::InitialScriptState;
use servo_constellation_traits::{SWManagerSenders, ServiceWorkerManagerFactory};
use servo_url::ImmutableOrigin;

/// Whether the hosted Constellation runs the full script-coupled browser
/// composition, or a script-free composition for viewer / static profiles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScriptingProfile {
    /// Full browser composition: real `script::ScriptThread` and
    /// `script::ServiceWorkerManager`. Default.
    #[default]
    Full,
    /// Script-free composition: pipelines spawn without instantiating a
    /// `ScriptThread`; service workers are not registered. Document/layout/
    /// paint still run; JavaScript execution does not.
    None,
}

/// `ScriptThreadFactory` impl that satisfies the trait without running any
/// script. The spawned thread exits immediately.
pub struct NoOpScriptThread;

impl ScriptThreadFactory for NoOpScriptThread {
    fn create(
        _state: InitialScriptState,
        _layout_factory: Arc<dyn LayoutFactory>,
        _image_cache_factory: Arc<dyn ImageCacheFactory>,
        _background_hang_monitor_register: Box<dyn BackgroundHangMonitorRegister>,
    ) -> JoinHandle<()> {
        thread::Builder::new()
            .name("NoOpScriptThread".to_owned())
            .spawn(|| {})
            .expect("failed to spawn no-op script thread")
    }
}

/// `ServiceWorkerManagerFactory` impl that does not register a manager.
pub struct NoOpServiceWorkerManager;

impl ServiceWorkerManagerFactory for NoOpServiceWorkerManager {
    fn create(_sw_senders: SWManagerSenders, _origin: ImmutableOrigin) {}
}
