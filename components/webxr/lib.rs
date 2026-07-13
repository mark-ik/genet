/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stub WebXR integration surface.
//!
//! The old implementation was built around GL/surfman. Genet keeps the
//! registry and discovery types available while WebXR is rebuilt on the future
//! wgpu/netrender path.

pub type MainThreadRegistry = webxr_api::MainThreadRegistry<()>;
pub type Discovery = Box<dyn webxr_api::DiscoveryAPI<()>>;

pub trait WebXrRegistry {
    /// Register services with a WebXR Registry.
    fn register(&self, _registry: &mut MainThreadRegistry) {}
}

pub mod glwindow {
    use std::rc::Rc;

    use webxr_api::{Error, Session, SessionBuilder, SessionInit, SessionMode};

    pub trait GlWindow {}

    pub struct GlWindowDiscovery {
        _window: Rc<dyn GlWindow>,
    }

    impl GlWindowDiscovery {
        pub fn new(window: Rc<dyn GlWindow>) -> Self {
            Self { _window: window }
        }
    }

    impl webxr_api::DiscoveryAPI<()> for GlWindowDiscovery {
        fn request_session(
            &mut self,
            _mode: SessionMode,
            _init: &SessionInit,
            _xr: SessionBuilder<()>,
        ) -> Result<Session, Error> {
            Err(Error::NoMatchingDevice)
        }

        fn supports_session(&self, _mode: SessionMode) -> bool {
            false
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub enum GlWindowMode {
        Blit,
        StereoLeftRight,
        StereoRedCyan,
        Cubemap,
        Spherical,
    }

    #[derive(Debug)]
    pub enum GlWindowRenderTarget {
        Unavailable,
    }
}

pub mod headless {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use servo_base::generic_channel::GenericReceiver;
    use webxr_api::{DiscoveryAPI, Error, MockDeviceInit, MockDeviceMsg};

    pub struct HeadlessMockDiscovery {
        _enabled: Arc<AtomicBool>,
    }

    impl HeadlessMockDiscovery {
        pub fn new(enabled: Arc<AtomicBool>) -> Self {
            Self { _enabled: enabled }
        }
    }

    impl webxr_api::MockDiscoveryAPI<()> for HeadlessMockDiscovery {
        fn simulate_device_connection(
            &mut self,
            _init: MockDeviceInit,
            _receiver: GenericReceiver<MockDeviceMsg>,
        ) -> Result<Box<dyn DiscoveryAPI<()>>, Error> {
            Err(Error::NoMatchingDevice)
        }
    }
}

pub mod openxr {
    use webxr_api::{Error, Session, SessionBuilder, SessionInit, SessionMode};

    pub trait ContextMenuProvider: Send {
        fn open_context_menu(&self) -> Box<dyn ContextMenuFuture>;
        fn clone_object(&self) -> Box<dyn ContextMenuProvider>;
    }

    pub trait ContextMenuFuture {
        fn poll(&self) -> ContextMenuResult;
    }

    pub enum ContextMenuResult {
        ExitSession,
        Dismissed,
        Pending,
    }

    #[derive(Default)]
    pub struct AppInfo {
        _application_name: String,
        _application_version: u32,
        _engine_name: String,
        _engine_version: u32,
    }

    impl AppInfo {
        pub fn new(
            application_name: &str,
            application_version: u32,
            engine_name: &str,
            engine_version: u32,
        ) -> AppInfo {
            Self {
                _application_name: application_name.to_string(),
                _application_version: application_version,
                _engine_name: engine_name.to_string(),
                _engine_version: engine_version,
            }
        }
    }

    pub struct OpenXrDiscovery {
        _context_menu_provider: Option<Box<dyn ContextMenuProvider>>,
        _app_info: AppInfo,
    }

    impl OpenXrDiscovery {
        pub fn new(
            context_menu_provider: Option<Box<dyn ContextMenuProvider>>,
            app_info: AppInfo,
        ) -> Self {
            Self {
                _context_menu_provider: context_menu_provider,
                _app_info: app_info,
            }
        }
    }

    impl webxr_api::DiscoveryAPI<()> for OpenXrDiscovery {
        fn request_session(
            &mut self,
            _mode: SessionMode,
            _init: &SessionInit,
            _xr: SessionBuilder<()>,
        ) -> Result<Session, Error> {
            Err(Error::NoMatchingDevice)
        }

        fn supports_session(&self, _mode: SessionMode) -> bool {
            false
        }
    }
}
