//! A GPU surface on a Wayland window.
//!
//! This crate holds the one piece of unsafe code that the editor has. The
//! driver of a GPU draws into a window through the window system's own
//! library, and has to be given that library's pointers to the display and
//! to the surface. Nothing in Rust's types says how long those pointers
//! are good for, so handing them over is unsafe, here as in every program
//! that draws with a GPU on Wayland.
//!
//! The rest of the workspace forbids unsafe code and uses [`WindowSurface`],
//! which is safe to make and to use.

use anyhow::{Context, Result, bail};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use std::ptr::NonNull;
use wayland_client::Proxy;
use wayland_client::protocol::wl_surface::WlSurface;

/// A surface that a GPU draws into, on a window.
pub struct WindowSurface {
    // Dropped in this order: the GPU's surface goes before what it is on.
    surface: wgpu::Surface<'static>,
    _window: WlSurface,
    _connection: wayland_client::Connection,
}

impl WindowSurface {
    /// Makes a surface for the GPU on a surface of the window system.
    pub fn new(
        instance: &wgpu::Instance,
        connection: &wayland_client::Connection,
        window: &WlSurface,
    ) -> Result<Self> {
        if !window.is_alive() {
            bail!("the window is gone");
        }
        let display = NonNull::new(connection.backend().display_ptr())
            .context("the connection to the compositor has no display")?;
        let proxy = NonNull::new(window.id().as_ptr()).context("the window has no surface")?;
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                display.cast(),
            ))),
            raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(proxy.cast())),
        };
        // SAFETY: the pointers must be valid now and until the surface made
        // from them is dropped.
        //
        // They are valid now: they come from a connection that is open and
        // from a surface that was just seen to be alive.
        //
        // The display stays valid because `WindowSurface` keeps the
        // connection, which owns the display, and drops it after the GPU's
        // surface.
        //
        // The surface stays valid as long as nobody destroys it. This
        // struct keeps a handle to it, which keeps the handle's own memory
        // but does not stop a `destroy` request sent through another
        // handle. The editor's window destroys its surface only when the
        // window itself is dropped, and the window owns this struct's
        // owner, so that this is dropped first. That last step is an
        // arrangement of the caller and not something this type enforces.
        #[allow(unsafe_code)]
        let surface = unsafe { instance.create_surface_unsafe(target) }
            .context("the GPU cannot draw on this window")?;
        Ok(Self {
            surface,
            _window: window.clone(),
            _connection: connection.clone(),
        })
    }

    pub fn surface(&self) -> &wgpu::Surface<'static> {
        &self.surface
    }
}
