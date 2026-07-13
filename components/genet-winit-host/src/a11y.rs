/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Platform AccessKit bridge for a genet-on-winit host.
//!
//! A host builds its accessibility tree however it likes (genet's
//! [`genet_layout::build_subtree`] projects a laid-out DOM; a host may stitch
//! several such subtrees) and hands the resulting [`TreeUpdate`] to
//! [`AccessKitBridge::update`]. The bridge owns the per-platform adapter
//! (`accesskit_windows` / `accesskit_macos` / `accesskit_unix`), pushes the tree
//! to the OS a11y API, and queues screen-reader [`ActionRequest`]s for the host to
//! drain and route back through its own activation paths. It is deliberately
//! host-agnostic: it knows about a winit [`Window`], a `TreeUpdate`, and a wake
//! callback, nothing about any particular host's panes or verbs.
//!
//! # Window lifecycle
//!
//! On Windows, [`AccessKitBridge::install`] creates a native subclassing
//! adapter and must run before the window is shown for the first time. Build an
//! initial tree while the window is hidden, install it, and then reveal the
//! window. Calling only [`AccessKitBridge::new`] is not sufficient, and hidden
//! winit windows may not receive the redraw that a deferred install expects.
//!
//! [`genet_layout::build_subtree`]: https://docs.rs/genet-layout

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[cfg(all(unix, not(target_os = "macos")))]
use accesskit::DeactivationHandler;
use accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, NodeId as AccessNodeId, TreeUpdate,
};

/// Whether the OS-level AccessKit adapter is live for this window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeStatus {
    Unavailable,
    Installed,
}

/// A screen reader's request to act on a node: the [`Action`] and the target
/// node's id. The host maps the id back to its own element and routes the action
/// through its activation path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct A11yActionRequest {
    pub action: Action,
    pub target_node: AccessNodeId,
}

struct Activation {
    latest: Arc<Mutex<Option<TreeUpdate>>>,
}

impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.latest.lock().ok().and_then(|latest| latest.clone())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
struct Deactivation;

#[cfg(all(unix, not(target_os = "macos")))]
impl DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {}
}

struct QueuedActions {
    actions: Arc<Mutex<VecDeque<A11yActionRequest>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl ActionHandler for QueuedActions {
    fn do_action(&mut self, request: ActionRequest) {
        if let Ok(mut actions) = self.actions.lock() {
            actions.push_back(A11yActionRequest {
                action: request.action,
                target_node: request.target_node,
            });
        }
        (self.wake)();
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use accesskit::TreeUpdate;
    use accesskit_windows::{HWND, SubclassingAdapter};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use super::{A11yActionRequest, Activation, BridgeStatus, QueuedActions};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    pub struct AccessKitBridge {
        adapter: Option<SubclassingAdapter>,
        latest: Arc<Mutex<Option<TreeUpdate>>>,
        actions: Arc<Mutex<VecDeque<A11yActionRequest>>>,
        wake: Arc<dyn Fn() + Send + Sync>,
    }

    impl AccessKitBridge {
        pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
            Self {
                adapter: None,
                latest: Arc::new(Mutex::new(None)),
                actions: Arc::new(Mutex::new(VecDeque::new())),
                wake: Arc::new(wake),
            }
        }

        pub fn status(&self) -> BridgeStatus {
            if self.adapter.is_some() {
                BridgeStatus::Installed
            } else {
                BridgeStatus::Unavailable
            }
        }

        pub fn install(&mut self, window: &Window, initial: TreeUpdate) -> Result<(), String> {
            *self.latest.lock().map_err(|err| err.to_string())? = Some(initial);
            if self.adapter.is_some() {
                return Ok(());
            }
            let hwnd = match window
                .window_handle()
                .map_err(|err| err.to_string())?
                .as_raw()
            {
                RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
                RawWindowHandle::WinRt(_) => {
                    return Err("WinRT window handles are not supported".to_string());
                }
                _ => return Err("window is not backed by a Win32 HWND".to_string()),
            };
            let adapter = SubclassingAdapter::new(
                hwnd,
                Activation {
                    latest: Arc::clone(&self.latest),
                },
                QueuedActions {
                    actions: Arc::clone(&self.actions),
                    wake: Arc::clone(&self.wake),
                },
            );
            self.adapter = Some(adapter);
            Ok(())
        }

        pub fn update(&mut self, update: TreeUpdate) {
            if let Ok(mut latest) = self.latest.lock() {
                *latest = Some(update.clone());
            }
            if let Some(adapter) = self.adapter.as_mut() {
                if let Some(events) = adapter.update_if_active(|| update) {
                    events.raise();
                }
            }
        }

        pub fn update_window_focus(&mut self, _focused: bool) {}

        pub fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
            let Ok(mut actions) = self.actions.lock() else {
                return Vec::new();
            };
            actions.drain(..).collect()
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use accesskit::TreeUpdate;
    use accesskit_macos::SubclassingAdapter;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use super::{A11yActionRequest, Activation, BridgeStatus, QueuedActions};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    pub struct AccessKitBridge {
        adapter: Option<SubclassingAdapter>,
        latest: Arc<Mutex<Option<TreeUpdate>>>,
        actions: Arc<Mutex<VecDeque<A11yActionRequest>>>,
        wake: Arc<dyn Fn() + Send + Sync>,
    }

    impl AccessKitBridge {
        pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
            Self {
                adapter: None,
                latest: Arc::new(Mutex::new(None)),
                actions: Arc::new(Mutex::new(VecDeque::new())),
                wake: Arc::new(wake),
            }
        }

        pub fn status(&self) -> BridgeStatus {
            if self.adapter.is_some() {
                BridgeStatus::Installed
            } else {
                BridgeStatus::Unavailable
            }
        }

        pub fn install(&mut self, window: &Window, initial: TreeUpdate) -> Result<(), String> {
            *self.latest.lock().map_err(|err| err.to_string())? = Some(initial);
            if self.adapter.is_some() {
                return Ok(());
            }
            let ns_view = match window
                .window_handle()
                .map_err(|err| err.to_string())?
                .as_raw()
            {
                RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr(),
                _ => return Err("window is not backed by an AppKit NSView".to_string()),
            };
            let adapter = unsafe {
                // SAFETY: `ns_view` comes from winit's live raw AppKit window handle
                // during `resumed`, while the window is owned by the application.
                SubclassingAdapter::new(
                    ns_view,
                    Activation {
                        latest: Arc::clone(&self.latest),
                    },
                    QueuedActions {
                        actions: Arc::clone(&self.actions),
                        wake: Arc::clone(&self.wake),
                    },
                )
            };
            self.adapter = Some(adapter);
            Ok(())
        }

        pub fn update(&mut self, update: TreeUpdate) {
            if let Ok(mut latest) = self.latest.lock() {
                *latest = Some(update.clone());
            }
            if let Some(adapter) = self.adapter.as_mut() {
                if let Some(events) = adapter.update_if_active(|| update) {
                    events.raise();
                }
            }
        }

        pub fn update_window_focus(&mut self, focused: bool) {
            if let Some(adapter) = self.adapter.as_mut() {
                if let Some(events) = adapter.update_view_focus_state(focused) {
                    events.raise();
                }
            }
        }

        pub fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
            let Ok(mut actions) = self.actions.lock() else {
                return Vec::new();
            };
            actions.drain(..).collect()
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use accesskit::{Rect, TreeUpdate};
    use accesskit_unix::Adapter;
    use winit::window::Window;

    use super::{A11yActionRequest, Activation, BridgeStatus, Deactivation, QueuedActions};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    pub struct AccessKitBridge {
        adapter: Option<Adapter>,
        latest: Arc<Mutex<Option<TreeUpdate>>>,
        actions: Arc<Mutex<VecDeque<A11yActionRequest>>>,
        wake: Arc<dyn Fn() + Send + Sync>,
    }

    impl AccessKitBridge {
        pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
            Self {
                adapter: None,
                latest: Arc::new(Mutex::new(None)),
                actions: Arc::new(Mutex::new(VecDeque::new())),
                wake: Arc::new(wake),
            }
        }

        pub fn status(&self) -> BridgeStatus {
            if self.adapter.is_some() {
                BridgeStatus::Installed
            } else {
                BridgeStatus::Unavailable
            }
        }

        pub fn install(&mut self, window: &Window, initial: TreeUpdate) -> Result<(), String> {
            *self.latest.lock().map_err(|err| err.to_string())? = Some(initial);
            if self.adapter.is_none() {
                let mut adapter = Adapter::new(
                    Activation {
                        latest: Arc::clone(&self.latest),
                    },
                    QueuedActions {
                        actions: Arc::clone(&self.actions),
                        wake: Arc::clone(&self.wake),
                    },
                    Deactivation,
                );
                set_root_window_bounds(&mut adapter, window);
                self.adapter = Some(adapter);
            }
            Ok(())
        }

        pub fn update(&mut self, update: TreeUpdate) {
            if let Ok(mut latest) = self.latest.lock() {
                *latest = Some(update.clone());
            }
            if let Some(adapter) = self.adapter.as_mut() {
                adapter.update_if_active(|| update);
            }
        }

        pub fn update_window_focus(&mut self, focused: bool) {
            if let Some(adapter) = self.adapter.as_mut() {
                adapter.update_window_focus_state(focused);
            }
        }

        pub fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
            let Ok(mut actions) = self.actions.lock() else {
                return Vec::new();
            };
            actions.drain(..).collect()
        }
    }

    fn set_root_window_bounds(adapter: &mut Adapter, window: &Window) {
        let outer_size = window.outer_size();
        let inner_size = window.inner_size();
        let outer = window.outer_position().ok();
        let inner = window.inner_position().ok();
        let outer_x = outer.map_or(0.0, |position| position.x as f64);
        let outer_y = outer.map_or(0.0, |position| position.y as f64);
        let inner_x = inner.map_or(outer_x, |position| position.x as f64);
        let inner_y = inner.map_or(outer_y, |position| position.y as f64);
        adapter.set_root_window_bounds(
            Rect::new(
                outer_x,
                outer_y,
                outer_x + outer_size.width as f64,
                outer_y + outer_size.height as f64,
            ),
            Rect::new(
                inner_x,
                inner_y,
                inner_x + inner_size.width as f64,
                inner_y + inner_size.height as f64,
            ),
        );
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
mod imp {
    use accesskit::TreeUpdate;
    use winit::window::Window;

    use super::{A11yActionRequest, BridgeStatus};

    pub struct AccessKitBridge;

    impl AccessKitBridge {
        pub fn new(_wake: impl Fn() + Send + Sync + 'static) -> Self {
            Self
        }

        pub fn status(&self) -> BridgeStatus {
            BridgeStatus::Unavailable
        }

        pub fn install(&mut self, _window: &Window, _initial: TreeUpdate) -> Result<(), String> {
            Err("AccessKit OS bridge is not wired for this platform".to_string())
        }

        pub fn update(&mut self, _update: TreeUpdate) {}

        pub fn update_window_focus(&mut self, _focused: bool) {}

        pub fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
            Vec::new()
        }
    }
}

pub use imp::AccessKitBridge;
