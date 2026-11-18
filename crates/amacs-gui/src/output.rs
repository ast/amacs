//! The two ways the window gets its picture: drawn on the CPU and copied
//! into memory shared with the compositor, or drawn by the GPU.

use amacs_gui::render::{Fonts, Gpu, Renderer, Software};
use amacs_proto::DrawOp;
use anyhow::Result;
use smithay_client_toolkit::reexports::client::Connection;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::shm::Shm;

use crate::present::ShmPresenter;

/// Which of the two a person asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RendererKind {
    #[default]
    Cpu,
    Gpu,
}

impl RendererKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "cpu" => Some(Self::Cpu),
            "gpu" => Some(Self::Gpu),
            _ => None,
        }
    }
}

pub enum Output {
    Cpu {
        renderer: Box<Renderer<Software>>,
        /// Made when the first picture is to be shown.
        presenter: Option<ShmPresenter>,
    },
    Gpu(Box<Renderer<Gpu>>),
}

impl Output {
    /// The output of the kind asked for. Where the GPU cannot be used, the
    /// CPU draws, and standard error says why.
    pub fn new(
        kind: RendererKind,
        fonts: Fonts,
        connection: &Connection,
        surface: &WlSurface,
    ) -> Self {
        if kind == RendererKind::Gpu {
            match Gpu::for_window(connection, surface) {
                Ok(gpu) => {
                    eprintln!("amacs-gui: drawing with {}", gpu.adapter());
                    return Self::Gpu(Box::new(Renderer::with_backend(fonts, gpu)));
                }
                Err(error) => {
                    eprintln!("amacs-gui: drawing on the CPU: {error:#}");
                }
            }
        }
        Self::Cpu {
            renderer: Box::new(Renderer::with_backend(fonts, Software::default())),
            presenter: None,
        }
    }

    pub fn apply(&mut self, ops: &[DrawOp]) -> bool {
        match self {
            Self::Cpu { renderer, .. } => renderer.apply(ops),
            Self::Gpu(renderer) => renderer.apply(ops),
        }
    }

    pub fn cell_size(&self) -> (u32, u32) {
        match self {
            Self::Cpu { renderer, .. } => renderer.cell_size(),
            Self::Gpu(renderer) => renderer.cell_size(),
        }
    }

    pub fn grid_size(&self, width: u32, height: u32) -> (u16, u16) {
        match self {
            Self::Cpu { renderer, .. } => renderer.grid_size(width, height),
            Self::Gpu(renderer) => renderer.grid_size(width, height),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        match self {
            Self::Cpu { renderer, .. } => renderer.resize(width, height),
            Self::Gpu(renderer) => renderer.resize(width, height),
        }
    }

    pub fn set_font_size(&mut self, font_px: f32) {
        match self {
            Self::Cpu { renderer, .. } => renderer.set_font_size(font_px),
            Self::Gpu(renderer) => renderer.set_font_size(font_px),
        }
    }

    /// Whether showing a picture also commits the surface, with whatever
    /// was asked of it before: a frame callback has then to be asked for
    /// before the picture is drawn, not after.
    pub fn commits(&self) -> bool {
        matches!(self, Self::Gpu(_))
    }

    /// Brings the surface's picture up to date. Returns whether there was
    /// anything new to show. For the CPU the surface is then ready to be
    /// committed; the GPU has committed it.
    pub fn show(&mut self, shm: &Shm, surface: &WlSurface) -> Result<bool> {
        match self {
            Self::Cpu {
                renderer,
                presenter,
            } => {
                let Some(damage) = renderer.paint() else {
                    return Ok(false);
                };
                let size = renderer.size();
                let presenter = match presenter {
                    Some(presenter) => presenter,
                    None => presenter.insert(ShmPresenter::new(shm, size)?),
                };
                Ok(presenter.present(surface, size, renderer.pixels(), damage))
            }
            Self::Gpu(renderer) => Ok(renderer.paint().is_some()),
        }
    }
}
