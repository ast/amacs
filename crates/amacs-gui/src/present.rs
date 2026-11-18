//! Getting a picture from memory onto the window's surface.
//!
//! This is the half of drawing that belongs to the software renderer: its
//! picture is copied into memory shared with the compositor. A renderer
//! that draws on the GPU would present through a swapchain of its own, in
//! a presenter beside this one.

use amacs_gui::render::Rect;
use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::client::protocol::{wl_shm, wl_surface};
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::SlotPool;

pub struct ShmPresenter {
    pool: SlotPool,
}

impl ShmPresenter {
    /// A presenter with memory for a picture of some size to begin with.
    pub fn new(shm: &Shm, (width, height): (u32, u32)) -> Result<Self> {
        let pool = SlotPool::new((width * height * 4) as usize, shm)
            .context("cannot create shared memory")?;
        Ok(Self { pool })
    }

    /// Attaches a picture of XRGB pixels to the surface and says what of it
    /// changed. Returns whether it did: the surface is then ready to be
    /// committed.
    pub fn present(
        &mut self,
        surface: &wl_surface::WlSurface,
        (width, height): (u32, u32),
        pixels: &[u32],
        damage: Rect,
    ) -> bool {
        let stride = width as i32 * 4;
        let Ok((buffer, canvas)) = self.pool.create_buffer(
            width as i32,
            height as i32,
            stride,
            wl_shm::Format::Xrgb8888,
        ) else {
            return false;
        };
        for (bytes, pixel) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
            *bytes = pixel.to_le_bytes();
        }
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.w as i32,
            damage.h as i32,
        );
        buffer.attach_to(surface).is_ok()
    }
}
