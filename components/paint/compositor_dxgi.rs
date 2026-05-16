/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Windows DXGI Composition `OsCompositorBackend` impl.
//!
//! Bridges netrender's wgpu master texture into a DirectComposition
//! visual tree attached to an HWND. Each declared
//! [`SurfaceKey`](netrender_device::compositor::SurfaceKey) maps to
//! one [`IDCompositionVisual`] in the tree; the
//! [`crate::interop::Dx12FenceSynchronizer`] gates the
//! consumer-side blit on netrender's render submit completing.
//!
//! ## Construction
//!
//! Call [`WindowsDxgiBackend::new`] with an HWND (the embedder's
//! window) and the [`HostWgpuContext`]. The backend creates a
//! `DCompositionDevice` from the same D3D12 device netrender uses,
//! a `Target` bound to the HWND, and a root visual that holds the
//! per-`SurfaceKey` children.
//!
//! ## Per-frame flow
//!
//! 1. [`Paint::render`] calls
//!    [`netrender::Renderer::render_with_compositor`].
//! 2. netrender records its submit on the wgpu D3D12 queue and signals
//!    the fence at `dx12_sync.advance()`.
//! 3. netrender's `Compositor::present_frame` fires; the
//!    [`crate::compositor::ServoCompositor::present_frame`] glue calls
//!    this backend's [`Self::present_master`] (and per-surface
//!    [`Self::present`] for declared compositor surfaces).
//! 4. The backend queue-waits the fence, blits the master into a
//!    DCOMP-bound texture, updates each visual's transform / clip /
//!    opacity, and `Commit`s the composition tree.
//!
//! ## Status
//!
//! The master/full-window DCOMP path is wired: `present_master` copies
//! the netrender master texture into the composition swapchain,
//! presents it, and commits the root visual tree. Per-[`SurfaceKey`]
//! presentation is also wired: `declare` creates a child visual with a
//! composition swapchain, and `present` copies the keyed destination
//! into that swapchain before updating transform / clip / opacity.

#![allow(unsafe_code)]

use rustc_hash::FxHashMap;
use wgpu::Texture;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Direct3D12::{
        D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
        D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST,
        D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_PRESENT,
        D3D12_RESOURCE_TRANSITION_BARRIER, ID3D12CommandAllocator, ID3D12CommandList,
        ID3D12CommandQueue, ID3D12Device, ID3D12GraphicsCommandList, ID3D12Resource,
    },
    Graphics::DirectComposition::{
        DCompositionCreateDevice, IDCompositionDevice, IDCompositionMatrixTransform,
        IDCompositionRectangleClip, IDCompositionTarget, IDCompositionVisual, IDCompositionVisual3,
    },
    Graphics::Dxgi::{
        Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC},
        CreateDXGIFactory2, DXGI_CREATE_FACTORY_DEBUG, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT,
        DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
        DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory4,
        IDXGISwapChain1,
    },
};
use windows::core::Interface;

use crate::compositor::OsCompositorBackend;
use crate::interop::{Dx12FenceSynchronizer, HostWgpuContext, InteropBackend, SyncMechanism};
use netrender_device::compositor::SurfaceKey;

/// Per-`SurfaceKey` DCOMP node. Holds the child visual, its
/// composition swapchain, and the destination texture the master
/// blits into.
struct DxgiSurface {
    visual: IDCompositionVisual,
    transform: IDCompositionMatrixTransform,
    clip: IDCompositionRectangleClip,
    swapchain: IDXGISwapChain1,
    cmd_allocator: ID3D12CommandAllocator,
    cmd_list: ID3D12GraphicsCommandList,
    /// Destination wgpu texture in D3D12-shared form. Sized to the
    /// surface's `world_bounds` at declare time; reallocated on
    /// resize.
    destination: Texture,
    size: (u32, u32),
}

/// Default initial swapchain size. Resized on the first
/// `present_master` call when the master texture's size is known.
const INITIAL_SIZE: u32 = 256;

/// Windows DXGI Composition backend. One per top-level window.
///
/// Construction allocates:
/// - A `DCompositionDevice` (OS-default; takes our wgpu D3D12 path
///   via the composition swapchain — DCOMP itself doesn't need a
///   handle to our D3D12 device)
/// - A `Target` bound to the embedder's HWND
/// - A root `Visual` that owns per-surface child visuals
/// - An `IDXGISwapChain1` in composition mode, attached to the
///   root visual via [`IDCompositionVisual::SetContent`]. The wgpu
///   D3D12 command queue is the swapchain's device queue, so
///   netrender's render submits land directly into the swapchain
///   without a cross-device blit.
/// - A [`Dx12FenceSynchronizer`] for the producer/consumer fence
///   dance when present moves to a dedicated consumer queue.
pub struct WindowsDxgiBackend {
    dcomp_device: IDCompositionDevice,
    target: IDCompositionTarget,
    root_visual: IDCompositionVisual,
    /// Composition swapchain. Backbuffer is the destination of the
    /// per-frame master copy.
    swapchain: IDXGISwapChain1,
    /// DXGI factory reused for per-surface composition swapchains.
    dxgi_factory: IDXGIFactory4,
    /// Current swapchain backbuffer dimensions. Reallocated on
    /// `ResizeBuffers` when the master changes shape.
    swapchain_size: (u32, u32),
    /// The wgpu D3D12 command queue. Cached at construction so the
    /// per-frame copy + present path doesn't re-pull it each call.
    /// This is the same queue netrender's submits run on, so the
    /// submit-then-copy ordering on this queue is naturally
    /// FIFO-correct without an explicit fence wait.
    queue: ID3D12CommandQueue,
    d3d_device: ID3D12Device,
    /// Reusable command allocator + list for the per-frame
    /// copy. Reset and re-recorded each call to `present_master`.
    cmd_allocator: ID3D12CommandAllocator,
    cmd_list: ID3D12GraphicsCommandList,
    surfaces: FxHashMap<SurfaceKey, DxgiSurface>,
    dx12_sync: Dx12FenceSynchronizer,
}

unsafe impl Send for WindowsDxgiBackend {}

impl WindowsDxgiBackend {
    /// Create a backend bound to `hwnd` over `host`'s wgpu D3D12
    /// device.
    ///
    /// Returns `Err` if `host.backend != InteropBackend::Dx12`, if the
    /// `IDXGIDevice` can't be obtained from the wgpu device, or if any
    /// DCOMP API call fails.
    pub fn new(host: &HostWgpuContext, hwnd: HWND) -> Result<Self, BackendError> {
        if host.backend != InteropBackend::Dx12 {
            return Err(BackendError::WrongBackend(host.backend));
        }

        let dx12_sync = Dx12FenceSynchronizer::new(host)
            .map_err(|e| BackendError::FenceInit(format!("{e}")))?;

        // Pull the wgpu D3D12 command queue out — the composition
        // swapchain needs it as its "device queue" so netrender's
        // submits land directly into the swapchain backbuffer.
        let d3d_queue: ID3D12CommandQueue = unsafe {
            let hal_queue = host
                .queue
                .as_hal::<wgpu::wgc::api::Dx12>()
                .ok_or(BackendError::NoHalDevice)?;
            let q = hal_queue.as_raw().clone();
            drop(hal_queue);
            q
        };

        // The wgpu D3D12 device. Used for the per-frame command
        // allocator + list allocation below.
        let d3d_device: ID3D12Device = unsafe {
            let hal_device = host
                .device
                .as_hal::<wgpu::wgc::api::Dx12>()
                .ok_or(BackendError::NoHalDevice)?;
            let d = hal_device.raw_device().clone();
            drop(hal_device);
            d
        };

        // Pre-allocate a one-shot command allocator + command list
        // that the per-frame copy resets and re-records into. Closed
        // immediately after creation so subsequent `Reset()` calls
        // succeed (a fresh command list opens in recording state and
        // must be `Close()`'d before its first `Reset()`).
        let cmd_allocator: ID3D12CommandAllocator = unsafe {
            d3d_device
                .CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .map_err(|e| BackendError::DComp(format!("CreateCommandAllocator: {e}")))?
        };
        let cmd_list: ID3D12GraphicsCommandList = unsafe {
            let list: ID3D12GraphicsCommandList = d3d_device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &cmd_allocator, None)
                .map_err(|e| BackendError::DComp(format!("CreateCommandList: {e}")))?;
            list.Close()
                .map_err(|e| BackendError::DComp(format!("CommandList::Close(initial): {e}")))?;
            list
        };

        // DCOMP device — `DCompositionCreateDevice(None)` tells DCOMP
        // to use the OS's default Direct3D device. That's the right
        // path for our setup: our composition swapchain owns the
        // wgpu D3D12 queue, and DCOMP's role is just to hand the
        // swapchain to the OS compositor via the visual tree.
        let dcomp_device: IDCompositionDevice = unsafe {
            let no_dxgi: Option<&IDXGIDevice> = None;
            DCompositionCreateDevice::<_, IDCompositionDevice>(no_dxgi)
                .map_err(|e| BackendError::DComp(format!("CreateDevice: {e}")))?
        };

        // DCOMP target attached to the HWND. `topmost = true` puts
        // the visual tree above the window's regular paint surface.
        let target: IDCompositionTarget = unsafe {
            dcomp_device
                .CreateTargetForHwnd(hwnd, true)
                .map_err(|e| BackendError::DComp(format!("CreateTargetForHwnd: {e}")))?
        };

        let root_visual: IDCompositionVisual = unsafe {
            dcomp_device
                .CreateVisual()
                .map_err(|e| BackendError::DComp(format!("CreateVisual(root): {e}")))?
        };

        // DXGI factory + composition swapchain.
        let dxgi_factory: IDXGIFactory4 = unsafe {
            CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_DEBUG)
                .or_else(|_| {
                    // Fall back to a non-debug factory if the debug
                    // layer isn't available (typical on shipping
                    // installs without the Windows SDK debug bits).
                    CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0))
                })
                .map_err(|e| BackendError::DComp(format!("CreateDXGIFactory2: {e}")))?
        };

        let swap_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: INITIAL_SIZE,
            Height: INITIAL_SIZE,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Stereo: false.into(),
            Flags: 0,
        };
        let swapchain: IDXGISwapChain1 = unsafe {
            dxgi_factory
                .CreateSwapChainForComposition(&d3d_queue, &swap_desc, None)
                .map_err(|e| BackendError::DComp(format!("CreateSwapChainForComposition: {e}")))?
        };

        unsafe {
            root_visual
                .SetContent(&swapchain)
                .map_err(|e| BackendError::DComp(format!("Visual::SetContent: {e}")))?;
            target
                .SetRoot(&root_visual)
                .map_err(|e| BackendError::DComp(format!("SetRoot: {e}")))?;
            dcomp_device
                .Commit()
                .map_err(|e| BackendError::DComp(format!("Commit(initial): {e}")))?;
        }

        Ok(Self {
            dcomp_device,
            target,
            root_visual,
            swapchain,
            dxgi_factory,
            swapchain_size: (INITIAL_SIZE, INITIAL_SIZE),
            queue: d3d_queue,
            d3d_device,
            cmd_allocator,
            cmd_list,
            surfaces: FxHashMap::default(),
            dx12_sync,
        })
    }

    /// Borrow of the Dx12 fence synchronizer. The producer (netrender)
    /// reads `current_value` after submit; the consumer (this
    /// backend's blit path) `queue_wait`s the fence before sampling.
    pub fn dx12_sync(&self) -> &Dx12FenceSynchronizer {
        &self.dx12_sync
    }

    fn create_composition_swapchain(
        &self,
        width: u32,
        height: u32,
        label: &str,
    ) -> Result<IDXGISwapChain1, BackendError> {
        let swap_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Stereo: false.into(),
            Flags: 0,
        };
        unsafe {
            self.dxgi_factory
                .CreateSwapChainForComposition(&self.queue, &swap_desc, None)
                .map_err(|e| {
                    BackendError::DComp(format!(
                        "CreateSwapChainForComposition({label}, {width}x{height}): {e}"
                    ))
                })
        }
    }

    fn create_command_pair(
        &self,
        label: &str,
    ) -> Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList), BackendError> {
        let allocator: ID3D12CommandAllocator = unsafe {
            self.d3d_device
                .CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .map_err(|e| BackendError::DComp(format!("CreateCommandAllocator({label}): {e}")))?
        };
        let list: ID3D12GraphicsCommandList = unsafe {
            let list: ID3D12GraphicsCommandList = self
                .d3d_device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                .map_err(|e| BackendError::DComp(format!("CreateCommandList({label}): {e}")))?;
            list.Close()
                .map_err(|e| BackendError::DComp(format!("CommandList::Close({label}): {e}")))?;
            list
        };
        Ok((allocator, list))
    }

    fn configure_surface_visual(
        surface: &DxgiSurface,
        clip: Option<[f32; 4]>,
    ) -> Result<(), BackendError> {
        let [left, top, right, bottom] =
            clip.unwrap_or([0.0, 0.0, surface.size.0 as f32, surface.size.1 as f32]);
        unsafe {
            surface
                .clip
                .SetLeft2(left)
                .map_err(|e| BackendError::DComp(format!("RectangleClip::SetLeft2: {e}")))?;
            surface
                .clip
                .SetTop2(top)
                .map_err(|e| BackendError::DComp(format!("RectangleClip::SetTop2: {e}")))?;
            surface
                .clip
                .SetRight2(right)
                .map_err(|e| BackendError::DComp(format!("RectangleClip::SetRight2: {e}")))?;
            surface
                .clip
                .SetBottom2(bottom)
                .map_err(|e| BackendError::DComp(format!("RectangleClip::SetBottom2: {e}")))?;
        }
        Ok(())
    }

    fn apply_surface_state(
        surface: &DxgiSurface,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) -> Result<(), BackendError> {
        let [m11, m12, m21, m22, dx, dy] = transform;
        unsafe {
            surface
                .transform
                .SetMatrixElement2(0, 0, m11)
                .map_err(|e| BackendError::DComp(format!("MatrixTransform::Set(0,0): {e}")))?;
            surface
                .transform
                .SetMatrixElement2(0, 1, m12)
                .map_err(|e| BackendError::DComp(format!("MatrixTransform::Set(0,1): {e}")))?;
            surface
                .transform
                .SetMatrixElement2(1, 0, m21)
                .map_err(|e| BackendError::DComp(format!("MatrixTransform::Set(1,0): {e}")))?;
            surface
                .transform
                .SetMatrixElement2(1, 1, m22)
                .map_err(|e| BackendError::DComp(format!("MatrixTransform::Set(1,1): {e}")))?;
            surface
                .transform
                .SetMatrixElement2(2, 0, dx)
                .map_err(|e| BackendError::DComp(format!("MatrixTransform::Set(2,0): {e}")))?;
            surface
                .transform
                .SetMatrixElement2(2, 1, dy)
                .map_err(|e| BackendError::DComp(format!("MatrixTransform::Set(2,1): {e}")))?;
            Self::configure_surface_visual(surface, clip)?;
            surface
                .visual
                .cast::<IDCompositionVisual3>()
                .map_err(|e| {
                    BackendError::DComp(format!("Visual cast to IDCompositionVisual3: {e}"))
                })?
                .SetOpacity2(opacity.clamp(0.0, 1.0))
                .map_err(|e| BackendError::DComp(format!("Visual3::SetOpacity2: {e}")))?;
        }
        Ok(())
    }

    fn present_surface(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) -> Result<(), BackendError> {
        let Some(surface) = self.surfaces.get(&key) else {
            return Ok(());
        };

        Self::apply_surface_state(surface, transform, clip, opacity)?;

        let source_d3d: ID3D12Resource = unsafe {
            let hal_tex = surface
                .destination
                .as_hal::<wgpu::wgc::api::Dx12>()
                .ok_or(BackendError::NoHalDevice)?;
            let r = hal_tex.raw_resource().clone();
            drop(hal_tex);
            r
        };
        let backbuffer: ID3D12Resource = unsafe {
            surface
                .swapchain
                .GetBuffer::<ID3D12Resource>(0)
                .map_err(|e| BackendError::DComp(format!("surface GetBuffer({key:?}): {e}")))?
        };

        unsafe {
            surface.cmd_allocator.Reset().map_err(|e| {
                BackendError::DComp(format!("surface Allocator::Reset({key:?}): {e}"))
            })?;
            surface
                .cmd_list
                .Reset(&surface.cmd_allocator, None)
                .map_err(|e| {
                    BackendError::DComp(format!("surface CommandList::Reset({key:?}): {e}"))
                })?;

            let pre = [
                transition_barrier(
                    &source_d3d,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
                transition_barrier(
                    &backbuffer,
                    D3D12_RESOURCE_STATE_PRESENT,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                ),
            ];
            surface.cmd_list.ResourceBarrier(&pre);

            surface.cmd_list.CopyResource(&backbuffer, &source_d3d);

            let post = [
                transition_barrier(
                    &backbuffer,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_PRESENT,
                ),
                transition_barrier(
                    &source_d3d,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                ),
            ];
            surface.cmd_list.ResourceBarrier(&post);

            surface.cmd_list.Close().map_err(|e| {
                BackendError::DComp(format!("surface CommandList::Close({key:?}): {e}"))
            })?;

            let lists: [Option<ID3D12CommandList>; 1] =
                [Some(surface.cmd_list.cast().map_err(|e| {
                    BackendError::DComp(format!("surface CommandList cast({key:?}): {e}"))
                })?)];
            self.queue.ExecuteCommandLists(&lists);

            surface
                .swapchain
                .Present(0, DXGI_PRESENT(0))
                .ok()
                .map_err(|e| BackendError::DComp(format!("surface Present({key:?}): {e}")))?;
            self.dcomp_device
                .Commit()
                .map_err(|e| BackendError::DComp(format!("surface Commit({key:?}): {e}")))?;
        }

        Ok(())
    }

    /// Present the netrender master texture into the composition
    /// swapchain. The master is copied into backbuffer 0, the
    /// swapchain is presented, and the DCOMP visual tree is
    /// committed so the OS compositor picks up the new frame.
    ///
    /// Same-queue flow: the wgpu D3D12 command queue holds both
    /// netrender's render submits AND this method's `CopyResource`,
    /// so submit-then-copy is naturally ordered without an explicit
    /// fence wait. The [`Dx12FenceSynchronizer::advance`] call
    /// preserves the fence-value protocol for any future multi-queue
    /// path.
    ///
    /// State barriers:
    /// - master: COMMON ↔ COPY_SOURCE (round-trip; wgpu's tracker view
    ///   of the master is unchanged after the call)
    /// - backbuffer: PRESENT → COPY_DEST → PRESENT
    pub fn present_master(&mut self, master: &Texture) -> Result<(), BackendError> {
        let _producer_value = self.dx12_sync.advance();
        let size = master.size();

        // Resize the swapchain to match the master if needed. The
        // composition swapchain stretches via `DXGI_SCALING_STRETCH`,
        // so size mismatch is non-fatal — but matching avoids the
        // extra GPU stretch and keeps pixel-perfect.
        if (size.width, size.height) != self.swapchain_size {
            unsafe {
                self.swapchain
                    .ResizeBuffers(
                        2,
                        size.width,
                        size.height,
                        DXGI_FORMAT_R8G8B8A8_UNORM,
                        DXGI_SWAP_CHAIN_FLAG(0),
                    )
                    .map_err(|e| BackendError::DComp(format!("ResizeBuffers: {e}")))?;
            }
            self.swapchain_size = (size.width, size.height);
        }

        // Pull the master's `ID3D12Resource` from wgpu-hal.
        let master_d3d: ID3D12Resource = unsafe {
            let hal_tex = master
                .as_hal::<wgpu::wgc::api::Dx12>()
                .ok_or(BackendError::NoHalDevice)?;
            let r = hal_tex.raw_resource().clone();
            drop(hal_tex);
            r
        };

        // Acquire the current backbuffer.
        let backbuffer: ID3D12Resource = unsafe {
            self.swapchain
                .GetBuffer::<ID3D12Resource>(0)
                .map_err(|e| BackendError::DComp(format!("GetBuffer(0): {e}")))?
        };

        // Record the copy.
        unsafe {
            self.cmd_allocator
                .Reset()
                .map_err(|e| BackendError::DComp(format!("Allocator::Reset: {e}")))?;
            self.cmd_list
                .Reset(&self.cmd_allocator, None)
                .map_err(|e| BackendError::DComp(format!("CommandList::Reset: {e}")))?;

            let pre = [
                transition_barrier(
                    &master_d3d,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
                transition_barrier(
                    &backbuffer,
                    D3D12_RESOURCE_STATE_PRESENT,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                ),
            ];
            self.cmd_list.ResourceBarrier(&pre);

            self.cmd_list.CopyResource(&backbuffer, &master_d3d);

            let post = [
                transition_barrier(
                    &backbuffer,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_PRESENT,
                ),
                transition_barrier(
                    &master_d3d,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_COMMON,
                ),
            ];
            self.cmd_list.ResourceBarrier(&post);

            self.cmd_list
                .Close()
                .map_err(|e| BackendError::DComp(format!("CommandList::Close: {e}")))?;

            let lists: [Option<ID3D12CommandList>; 1] =
                [Some(self.cmd_list.cast().map_err(|e| {
                    BackendError::DComp(format!("CommandList cast: {e}"))
                })?)];
            self.queue.ExecuteCommandLists(&lists);

            self.swapchain
                .Present(0, DXGI_PRESENT(0))
                .ok()
                .map_err(|e| BackendError::DComp(format!("Present: {e}")))?;
            self.dcomp_device
                .Commit()
                .map_err(|e| BackendError::DComp(format!("Commit(frame): {e}")))?;
        }

        let _ = (&self.target, &self.root_visual);
        Ok(())
    }
}

/// Helper to construct a `D3D12_RESOURCE_BARRIER` for a single-resource
/// transition. Uses `BarrierFlag::None`, `Subresource = u32::MAX`
/// (transition-all-subresources).
fn transition_barrier(
    resource: &ID3D12Resource,
    before: windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES,
    after: windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
                Subresource: u32::MAX,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

impl OsCompositorBackend for WindowsDxgiBackend {
    fn interop_backend(&self) -> InteropBackend {
        InteropBackend::Dx12
    }

    fn sync_mechanism(&self) -> SyncMechanism {
        SyncMechanism::ExplicitFence
    }

    fn present_master(&mut self, master: &Texture) {
        if let Err(err) = WindowsDxgiBackend::present_master(self, master) {
            log::warn!("[WindowsDxgiBackend] present_master failed: {err}");
        }
    }

    fn declare(
        &mut self,
        key: SurfaceKey,
        host: &crate::interop::HostWgpuContext,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::Texture, crate::compositor::BoxedBackendError> {
        // Allocate the destination wgpu texture (same shape the
        // trait default uses). Then do the OS-side bookkeeping:
        // create a child `IDCompositionVisual`, attach a composition
        // swapchain as its content, and add it above the master root
        // content so `present` can update transform/clip/opacity and
        // copy the destination into the child swapchain each frame.
        let destination = host.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("WindowsDxgiBackend surface destination"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let visual = unsafe { self.dcomp_device.CreateVisual() }
            .map_err(|e| Box::new(BackendError::DComp(format!("CreateVisual: {e}"))))?;
        let transform = unsafe { self.dcomp_device.CreateMatrixTransform() }
            .map_err(|e| Box::new(BackendError::DComp(format!("CreateMatrixTransform: {e}"))))?;
        let clip = unsafe { self.dcomp_device.CreateRectangleClip() }
            .map_err(|e| Box::new(BackendError::DComp(format!("CreateRectangleClip: {e}"))))?;
        let swapchain = self
            .create_composition_swapchain(width, height, "surface")
            .map_err(Box::new)?;
        let (cmd_allocator, cmd_list) = self.create_command_pair("surface").map_err(Box::new)?;
        unsafe {
            transform.SetMatrixElement2(0, 0, 1.0).map_err(|e| {
                Box::new(BackendError::DComp(format!(
                    "MatrixTransform::Set(0,0): {e}"
                )))
            })?;
            transform.SetMatrixElement2(1, 1, 1.0).map_err(|e| {
                Box::new(BackendError::DComp(format!(
                    "MatrixTransform::Set(1,1): {e}"
                )))
            })?;
            clip.SetLeft2(0.0).map_err(|e| {
                Box::new(BackendError::DComp(format!("RectangleClip::SetLeft2: {e}")))
            })?;
            clip.SetTop2(0.0).map_err(|e| {
                Box::new(BackendError::DComp(format!("RectangleClip::SetTop2: {e}")))
            })?;
            clip.SetRight2(width as f32).map_err(|e| {
                Box::new(BackendError::DComp(format!(
                    "RectangleClip::SetRight2: {e}"
                )))
            })?;
            clip.SetBottom2(height as f32).map_err(|e| {
                Box::new(BackendError::DComp(format!(
                    "RectangleClip::SetBottom2: {e}"
                )))
            })?;
            visual.SetContent(&swapchain).map_err(|e| {
                Box::new(BackendError::DComp(format!(
                    "Visual::SetContent(surface): {e}"
                )))
            })?;
            visual.SetTransform(&transform).map_err(|e| {
                Box::new(BackendError::DComp(format!(
                    "Visual::SetTransform(surface): {e}"
                )))
            })?;
            visual.SetClip(&clip).map_err(|e| {
                Box::new(BackendError::DComp(format!(
                    "Visual::SetClip(surface): {e}"
                )))
            })?;
            visual
                .cast::<IDCompositionVisual3>()
                .map_err(|e| {
                    Box::new(BackendError::DComp(format!(
                        "Visual cast to IDCompositionVisual3: {e}"
                    )))
                })?
                .SetOpacity2(1.0)
                .map_err(|e| {
                    Box::new(BackendError::DComp(format!(
                        "Visual3::SetOpacity2(surface): {e}"
                    )))
                })?;
            self.root_visual
                .AddVisual(&visual, true, None::<&IDCompositionVisual>)
                .map_err(|e| {
                    Box::new(BackendError::DComp(format!(
                        "RootVisual::AddVisual(surface): {e}"
                    )))
                })?;
            self.dcomp_device.Commit().map_err(|e| {
                Box::new(BackendError::DComp(format!("Commit(declare surface): {e}")))
            })?;
        }
        self.surfaces.insert(
            key,
            DxgiSurface {
                visual,
                transform,
                clip,
                swapchain,
                cmd_allocator,
                cmd_list,
                destination: destination.clone(),
                size: (width, height),
            },
        );
        Ok(destination)
    }

    fn destroy(&mut self, key: SurfaceKey) {
        if let Some(surface) = self.surfaces.remove(&key) {
            unsafe {
                if let Err(err) = self.root_visual.RemoveVisual(&surface.visual) {
                    log::warn!("[WindowsDxgiBackend] RemoveVisual({key:?}) failed: {err}");
                }
                if let Err(err) = self.dcomp_device.Commit() {
                    log::warn!("[WindowsDxgiBackend] Commit(destroy {key:?}) failed: {err}");
                }
            }
        }
    }

    fn present(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) {
        if let Err(err) = self.present_surface(key, transform, clip, opacity) {
            log::warn!("[WindowsDxgiBackend] present({key:?}) failed: {err}");
        }
    }
}

/// Errors raised by [`WindowsDxgiBackend::new`].
#[derive(Debug)]
pub enum BackendError {
    /// The supplied host wgpu context is not running on D3D12.
    WrongBackend(InteropBackend),
    /// Failed to obtain the wgpu-hal D3D12 device.
    NoHalDevice,
    /// The Dx12 fence synchronizer could not be initialised.
    FenceInit(String),
    /// A DirectComposition API call failed.
    DComp(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongBackend(b) => write!(f, "WindowsDxgiBackend requires Dx12, found {b:?}"),
            Self::NoHalDevice => {
                f.write_str("WindowsDxgiBackend: wgpu-hal Dx12 device unavailable")
            },
            Self::FenceInit(m) => write!(f, "WindowsDxgiBackend: fence init failed: {m}"),
            Self::DComp(m) => write!(f, "WindowsDxgiBackend: DComp call failed: {m}"),
        }
    }
}

impl std::error::Error for BackendError {}
