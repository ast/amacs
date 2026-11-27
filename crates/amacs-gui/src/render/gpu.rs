//! Drawing on the GPU.
//!
//! Every paint draws the whole frame: each rectangle and glyph of the
//! display list is one instance of a quad. The pictures of glyphs are kept
//! in one texture, the atlas, and a glyph's quad is a piece of it.
//!
//! Colours are blended as the numbers they are, without regard to gamma,
//! which is what drawing on the CPU does too: the two backends give the
//! same picture, but for rounding.
//!
//! A glyph drawn per subpixel covers the red, green and blue of a pixel by
//! different amounts, which takes blending with two sources: the colour,
//! and the coverage of each channel. A GPU without that draws such glyphs
//! in shades of grey.

use std::collections::HashMap;

use amacs_gpu_surface::WindowSurface;
use amacs_proto::{Color, ImageId};
use anyhow::{Context, Result, bail};
use wayland_client::Connection;
use wayland_client::protocol::wl_surface::WlSurface;

use super::list::{Frame, Row};
use super::scene::{Image, Images};
use super::text::{GlyphContent, GlyphKey, GlyphSource};
use super::{Backend, Rect};

const SHADER: &str = r"
struct Globals {
    size: vec2<f32>,
    pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var pictures: texture_2d<f32>;

struct Instance {
    // In pixels: left, top, width, height.
    @location(0) rect: vec4<f32>,
    // In texels of the atlas: left, top, width, height.
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    // 0: a rectangle of the colour. 1: a glyph that is a mask for the
    // colour. 2: a glyph with colours of its own. 3: a glyph that is a
    // mask for each channel of the colour. 4: a piece of a picture, whose
    // texels are those of the other texture.
    @location(3) kind: vec4<f32>,
};

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: f32,
};

@vertex
fn vertex(@builtin(vertex_index) index: u32, instance: Instance) -> Corner {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let pixel = instance.rect.xy + corner * instance.rect.zw;
    var out: Corner;
    out.position = vec4<f32>(
        pixel.x / globals.size.x * 2.0 - 1.0,
        1.0 - pixel.y / globals.size.y * 2.0,
        0.0,
        1.0,
    );
    out.uv = instance.uv.xy + corner * instance.uv.zw;
    out.color = instance.color;
    out.kind = instance.kind.x;
    return out;
}

@fragment
fn fragment(corner: Corner) -> @location(0) vec4<f32> {
    if corner.kind < 0.5 {
        return corner.color;
    }
    if corner.kind > 3.5 {
        return textureLoad(pictures, vec2<i32>(floor(corner.uv)), 0);
    }
    let texel = textureLoad(atlas, vec2<i32>(floor(corner.uv)), 0);
    if corner.kind < 1.5 {
        return vec4<f32>(corner.color.rgb, texel.a);
    }
    return texel;
}
";

/// The fragment shader for blending with two sources: the colour, and how
/// much of each channel of the pixel it covers.
const DUAL_SOURCE_FRAGMENT: &str = r"
struct Fragments {
    @location(0) @blend_src(0) color: vec4<f32>,
    @location(0) @blend_src(1) coverage: vec4<f32>,
};

@fragment
fn fragment_dual(corner: Corner) -> Fragments {
    var out: Fragments;
    out.color = corner.color;
    if corner.kind < 0.5 {
        out.coverage = vec4<f32>(corner.color.a);
        return out;
    }
    if corner.kind > 3.5 {
        out.color = textureLoad(pictures, vec2<i32>(floor(corner.uv)), 0);
        out.coverage = vec4<f32>(out.color.a);
        return out;
    }
    let texel = textureLoad(atlas, vec2<i32>(floor(corner.uv)), 0);
    if corner.kind < 1.5 {
        out.coverage = vec4<f32>(texel.a);
        return out;
    }
    if corner.kind < 2.5 {
        out.color = texel;
        out.coverage = vec4<f32>(texel.a);
        return out;
    }
    out.coverage = vec4<f32>(texel.rgb, 1.0);
    return out;
}
";

/// The whole shader, with the two-source fragment shader where the GPU can
/// blend that way. The `enable` has to come first.
fn shader_source(dual_source: bool) -> String {
    if dual_source {
        format!("enable dual_source_blending;\n{SHADER}{DUAL_SOURCE_FRAGMENT}")
    } else {
        SHADER.to_owned()
    }
}

/// Floats in an instance: four vectors of four.
const INSTANCE_FLOATS: usize = 16;
const INSTANCE_BYTES: u64 = (INSTANCE_FLOATS * 4) as u64;

/// The atlas is square, with a side of at most this many texels.
const ATLAS_SIDE: u32 = 2048;

/// Where a glyph's picture is in the atlas, and what it is.
#[derive(Clone, Copy)]
struct Slot {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    /// Where the left and the top of the picture are from the glyph's
    /// origin: to the right, and upward.
    left: i32,
    top: i32,
    content: GlyphContent,
}

/// The pictures of glyphs in one texture, packed in rows from the top.
struct Atlas {
    texture: wgpu::Texture,
    side: u32,
    /// Whether glyphs drawn per subpixel can be blended as such.
    dual_source: bool,
    /// Glyphs without a picture are remembered too.
    slots: HashMap<GlyphKey, Option<Slot>>,
    /// Where the next picture goes, and how tall the row it is in has
    /// become.
    next: (u32, u32),
    row_height: u32,
}

impl Atlas {
    fn new(device: &wgpu::Device, dual_source: bool) -> Self {
        let side = ATLAS_SIDE.min(device.limits().max_texture_dimension_2d);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Self {
            texture,
            side,
            dual_source,
            slots: HashMap::new(),
            next: (0, 0),
            row_height: 0,
        }
    }

    fn clear(&mut self) {
        self.slots.clear();
        self.next = (0, 0);
        self.row_height = 0;
    }

    /// The place of a glyph's picture, which is put in the atlas if it is
    /// not there. `Err` if the atlas is full.
    fn slot(
        &mut self,
        queue: &wgpu::Queue,
        glyphs: &mut dyn GlyphSource,
        key: GlyphKey,
    ) -> Result<Option<Slot>, Full> {
        if let Some(slot) = self.slots.get(&key) {
            return Ok(*slot);
        }
        let slot = match glyphs.image(key) {
            Some(image)
                if image.width > 0
                    && image.height > 0
                    && image.width <= self.side
                    && image.height <= self.side =>
            {
                let texels = (image.width * image.height) as usize;
                let mut content = image.content;
                let rgba: Vec<u8> = match image.content {
                    GlyphContent::Mask => image
                        .data
                        .iter()
                        .take(texels)
                        .flat_map(|coverage| [255, 255, 255, *coverage])
                        .collect(),
                    GlyphContent::Color => image.data.iter().take(texels * 4).copied().collect(),
                    GlyphContent::Subpixel if self.dual_source => image
                        .data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .take(texels)
                        .flat_map(|parts| [parts[0], parts[1], parts[2], 255])
                        .collect(),
                    // Without two-source blending, the mean of the three
                    // parts is the coverage of the whole pixel.
                    GlyphContent::Subpixel => {
                        content = GlyphContent::Mask;
                        image
                            .data
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .take(texels)
                            .flat_map(|parts| {
                                let sum: u32 = parts[..3].iter().map(|&part| u32::from(part)).sum();
                                [255, 255, 255, (sum / 3) as u8]
                            })
                            .collect()
                    }
                };
                if rgba.len() != texels * 4 {
                    // A picture with less data than its size says is not one.
                    None
                } else {
                    if self.next.0 + image.width > self.side {
                        self.next = (0, self.next.1 + self.row_height);
                        self.row_height = 0;
                    }
                    if self.next.1 + image.height > self.side {
                        return Err(Full);
                    }
                    let (x, y) = self.next;
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x, y, z: 0 },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &rgba,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(image.width * 4),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width: image.width,
                            height: image.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    self.next.0 += image.width;
                    self.row_height = self.row_height.max(image.height);
                    Some(Slot {
                        x,
                        y,
                        width: image.width,
                        height: image.height,
                        left: image.left,
                        top: image.top,
                        content,
                    })
                }
            }
            _ => None,
        };
        self.slots.insert(key, slot);
        Ok(slot)
    }
}

/// The atlas has no room for another glyph.
struct Full;

/// The texture of pictures is square, with a side of at most this many
/// texels: a few screens of them.
const PICTURES_SIDE: u32 = 4096;

/// The pictures that rows show, in one texture, packed in rows from the top.
struct Pictures {
    texture: wgpu::Texture,
    side: u32,
    /// Where each picture is: left, top, width, height. A picture too
    /// large for the texture is remembered as not there.
    slots: HashMap<ImageId, Option<[u32; 4]>>,
    next: (u32, u32),
    row_height: u32,
}

impl Pictures {
    fn new(device: &wgpu::Device) -> Self {
        let side = PICTURES_SIDE.min(device.limits().max_texture_dimension_2d);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pictures"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Self {
            texture,
            side,
            slots: HashMap::new(),
            next: (0, 0),
            row_height: 0,
        }
    }

    fn clear(&mut self) {
        self.slots.clear();
        self.next = (0, 0);
        self.row_height = 0;
    }

    /// The place of a picture, which is put in the texture if it is not
    /// there. `Err` if the texture is full.
    fn slot(
        &mut self,
        queue: &wgpu::Queue,
        id: ImageId,
        image: &Image,
    ) -> Result<Option<[u32; 4]>, Full> {
        if let Some(slot) = self.slots.get(&id) {
            return Ok(*slot);
        }
        let fits = image.width > 0
            && image.height > 0
            && image.width <= self.side
            && image.height <= self.side;
        if !fits {
            self.slots.insert(id, None);
            return Ok(None);
        }
        if self.next.0 + image.width > self.side {
            self.next = (0, self.next.1 + self.row_height);
            self.row_height = 0;
        }
        if self.next.1 + image.height > self.side {
            return Err(Full);
        }
        let (x, y) = self.next;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );
        self.next.0 += image.width;
        self.row_height = self.row_height.max(image.height);
        let slot = Some([x, y, image.width, image.height]);
        self.slots.insert(id, slot);
        Ok(slot)
    }
}

/// What the GPU draws into.
enum Target {
    Window {
        surface: WindowSurface,
        config: wgpu::SurfaceConfiguration,
    },
    /// A picture in the GPU's memory, for tests and for looking at.
    Texture(Option<wgpu::Texture>),
}

pub struct Gpu {
    /// The name of the GPU and how it is driven, for a person.
    adapter: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    /// Draws white where there was black and so on: the cursor.
    invert: wgpu::RenderPipeline,
    /// Whether the GPU blends with two sources, which glyphs drawn per
    /// subpixel need to be shown as such.
    dual_source: bool,
    globals: wgpu::Buffer,
    bindings: wgpu::BindGroup,
    instances: wgpu::Buffer,
    /// How many instances the buffer has room for.
    capacity: u64,
    atlas: Atlas,
    pictures: Pictures,
    width: u32,
    height: u32,
    drawn_cursor: Option<Rect>,
    /// What is shown is not what was last painted, as after a resize.
    lost: bool,
}

fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle())
}

impl Gpu {
    /// A backend that draws into a window.
    pub fn for_window(connection: &Connection, window: &WlSurface) -> Result<Self> {
        let instance = instance();
        let surface = WindowSurface::new(&instance, connection, window)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(surface.surface()),
            ..wgpu::RequestAdapterOptions::default()
        }))
        .context("there is no GPU that can draw on this window")?;
        let capabilities = surface.surface().get_capabilities(&adapter);
        // Colours are blended as numbers, so the format must not convert.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().map(|f| f.remove_srgb_suffix()))
            .context("the GPU offers no format for this window")?;
        // The window paces its own drawing by the compositor's word, so
        // presenting must never wait.
        let present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let alpha_mode = if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::Opaque)
        {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            wgpu::CompositeAlphaMode::Auto
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 1,
            height: 1,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: Vec::new(),
        };
        Self::with_target(&adapter, Target::Window { surface, config }, format)
    }

    /// A backend that draws into a picture in the GPU's memory, which
    /// [`Gpu::read_pixels`] fetches.
    pub fn offscreen() -> Result<Self> {
        let instance = instance();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .context("there is no GPU")?;
        Self::with_target(
            &adapter,
            Target::Texture(None),
            wgpu::TextureFormat::Bgra8Unorm,
        )
    }

    fn with_target(
        adapter: &wgpu::Adapter,
        target: Target,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let dual_source = adapter
            .features()
            .contains(wgpu::Features::DUAL_SOURCE_BLENDING);
        let required_features = if dual_source {
            wgpu::Features::DUAL_SOURCE_BLENDING
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features,
            ..wgpu::DeviceDescriptor::default()
        }))
        .context("the GPU cannot be used")?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quads"),
            source: wgpu::ShaderSource::Wgsl(shader_source(dual_source).into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quads"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quads"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x4,
            1 => Float32x4,
            2 => Float32x4,
            3 => Float32x4,
        ];
        let pipeline = |label: &str, entry_point: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: INSTANCE_BYTES,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // What is drawn covers what is there by its alpha, colour by colour.
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        // White drawn this way leaves one minus what was there.
        let inverse = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        };
        // The same, channel by channel, by the second source.
        let over_each = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Src1,
            dst_factor: wgpu::BlendFactor::OneMinusSrc1,
            operation: wgpu::BlendOperation::Add,
        };
        let over_each_alpha = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Src1Alpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrc1Alpha,
            operation: wgpu::BlendOperation::Add,
        };
        let normal = if dual_source {
            pipeline(
                "quads",
                "fragment_dual",
                wgpu::BlendState {
                    color: over_each,
                    alpha: over_each_alpha,
                },
            )
        } else {
            pipeline(
                "quads",
                "fragment",
                wgpu::BlendState {
                    color: over,
                    alpha: over,
                },
            )
        };
        let invert = pipeline(
            "cursor",
            "fragment",
            wgpu::BlendState {
                color: inverse,
                alpha: inverse,
            },
        );

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let atlas = Atlas::new(&device, dual_source);
        let pictures = Pictures::new(&device);
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quads"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &atlas
                            .texture
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &pictures
                            .texture
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
            ],
        });
        let capacity = 4096;
        let instances = Self::instance_buffer(&device, capacity);
        let info = adapter.get_info();
        Ok(Self {
            adapter: format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend),
            device,
            queue,
            target,
            format,
            pipeline: normal,
            invert,
            dual_source,
            globals,
            bindings,
            instances,
            capacity,
            atlas,
            pictures,
            width: 0,
            height: 0,
            drawn_cursor: None,
            lost: true,
        })
    }

    /// Waits until the GPU has done all that it was asked to, as for
    /// measuring how long that takes.
    pub fn finish(&self) -> Result<()> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("the GPU did not finish")?;
        Ok(())
    }

    /// The name of the GPU and how it is driven, for a person.
    /// Whether glyphs drawn per subpixel are shown as such. A GPU that
    /// cannot blend with two sources shows them in shades of grey.
    pub fn draws_subpixels(&self) -> bool {
        self.dual_source
    }

    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: capacity * INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// The instances for every row of a frame, and after them one for the
    /// cursor if there is one. `Err` if the atlas filled up on the way.
    fn build(&mut self, frame: &Frame, glyphs: &mut dyn GlyphSource) -> Result<Vec<f32>, Full> {
        let mut out = Vec::new();
        for row in frame.rows {
            self.build_row(row, frame.images, glyphs, &mut out)?;
        }
        if let Some(cell) = frame.cursor {
            push(&mut out, quad(cell), [0.0; 4], [1.0; 4], 0.0);
        }
        Ok(out)
    }

    fn build_row(
        &mut self,
        row: &Row,
        images: &Images,
        glyphs: &mut dyn GlyphSource,
        out: &mut Vec<f32>,
    ) -> Result<(), Full> {
        for background in &row.backgrounds {
            push(
                out,
                quad(background.rect),
                [0.0; 4],
                rgba(background.color),
                0.0,
            );
        }
        // Nothing of a glyph shows outside its row or the picture.
        let clip = (
            row.area.x as i32,
            row.area.y as i32,
            (row.area.x + row.area.w).min(self.width) as i32,
            (row.area.y + row.area.h).min(self.height) as i32,
        );
        for picture in &row.images {
            let Some(image) = images.get(&picture.id) else {
                continue;
            };
            let Some([slot_x, slot_y, width, height]) =
                self.pictures.slot(&self.queue, picture.id, image)?
            else {
                continue;
            };
            let (x0, y0) = (picture.x.max(clip.0), picture.y.max(clip.1));
            let x1 = (picture.x + picture.w as i32).min(clip.2);
            let y1 = (picture.y + picture.h as i32).min(clip.3);
            if x0 >= x1 || y0 >= y1 || picture.w == 0 || picture.h == 0 {
                continue;
            }
            // So many texels of the picture to a pixel of the screen.
            let (step_x, step_y) = (
                width as f32 / picture.w as f32,
                height as f32 / picture.h as f32,
            );
            let rect = [x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32];
            let uv = [
                slot_x as f32 + (x0 - picture.x) as f32 * step_x,
                slot_y as f32 + (y0 - picture.y) as f32 * step_y,
                rect[2] * step_x,
                rect[3] * step_y,
            ];
            push(out, rect, uv, [1.0; 4], 4.0);
        }
        for glyph in &row.glyphs {
            let Some(slot) = self.atlas.slot(&self.queue, glyphs, glyph.key)? else {
                continue;
            };
            let (left, top) = (glyph.x + slot.left, glyph.y - slot.top);
            let (x0, y0) = (left.max(clip.0), top.max(clip.1));
            let x1 = (left + slot.width as i32).min(clip.2);
            let y1 = (top + slot.height as i32).min(clip.3);
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let rect = [x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32];
            let uv = [
                (slot.x as i32 + x0 - left) as f32,
                (slot.y as i32 + y0 - top) as f32,
                rect[2],
                rect[3],
            ];
            let kind = match slot.content {
                GlyphContent::Mask => 1.0,
                GlyphContent::Color => 2.0,
                GlyphContent::Subpixel => 3.0,
            };
            push(out, rect, uv, rgba(glyph.color), kind);
        }
        for overlay in &row.overlays {
            push(out, quad(overlay.rect), [0.0; 4], rgba(overlay.color), 0.0);
        }
        Ok(())
    }

    /// The picture that was drawn into memory, one XRGB `u32` per pixel,
    /// row by row, as the software backend keeps its own. Empty for a
    /// backend that draws into a window.
    pub fn read_pixels(&self) -> Result<Vec<u32>> {
        let Target::Texture(Some(texture)) = &self.target else {
            return Ok(Vec::new());
        };
        // Rows in a buffer start at multiples of 256 bytes.
        let stride = (self.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("picture"),
            size: u64::from(stride) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("the GPU did not finish")?;
        receiver
            .recv()
            .context("the GPU gave no answer")?
            .context("the picture cannot be read")?;
        let bytes = buffer
            .slice(..)
            .get_mapped_range()
            .context("the picture cannot be read")?;
        if self.format != wgpu::TextureFormat::Bgra8Unorm {
            bail!("the picture is in a format that is not read here");
        }
        let mut pixels = Vec::with_capacity(self.width as usize * self.height as usize);
        for row in bytes.chunks(stride as usize) {
            for pixel in row[..self.width as usize * 4].as_chunks::<4>().0 {
                pixels.push(
                    0xff00_0000
                        | u32::from(pixel[2]) << 16
                        | u32::from(pixel[1]) << 8
                        | u32::from(pixel[0]),
                );
            }
        }
        Ok(pixels)
    }

    /// Draws the instances into a view of the target.
    fn draw(&mut self, view: &wgpu::TextureView, frame: &Frame, floats: &[f32]) {
        let count = (floats.len() / INSTANCE_FLOATS) as u64;
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.instances = Self::instance_buffer(&self.device, self.capacity);
        }
        let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
        self.queue.write_buffer(&self.instances, 0, &bytes);
        let size: Vec<u8> = [self.width as f32, self.height as f32, 0.0, 0.0]
            .iter()
            .flat_map(|f| f.to_ne_bytes())
            .collect();
        self.queue.write_buffer(&self.globals, 0, &size);

        // The cursor is the last instance, and is drawn another way.
        let cursor = u64::from(frame.cursor.is_some());
        let background = rgba(frame.background);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(background[0]),
                            g: f64::from(background[1]),
                            b: f64::from(background[2]),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..wgpu::RenderPassDescriptor::default()
            });
            pass.set_bind_group(0, &self.bindings, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.set_pipeline(&self.pipeline);
            pass.draw(0..4, 0..(count - cursor) as u32);
            if cursor > 0 {
                pass.set_pipeline(&self.invert);
                pass.draw(0..4, (count - 1) as u32..count as u32);
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}

fn rgba(color: Color) -> [f32; 4] {
    [
        f32::from(color.r) / 255.0,
        f32::from(color.g) / 255.0,
        f32::from(color.b) / 255.0,
        1.0,
    ]
}

fn quad(rect: Rect) -> [f32; 4] {
    [rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32]
}

fn push(out: &mut Vec<f32>, rect: [f32; 4], uv: [f32; 4], color: [f32; 4], kind: f32) {
    out.extend_from_slice(&rect);
    out.extend_from_slice(&uv);
    out.extend_from_slice(&color);
    out.extend_from_slice(&[kind, 0.0, 0.0, 0.0]);
}

impl Backend for Gpu {
    fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.lost = true;
        if width == 0 || height == 0 {
            return;
        }
        match &mut self.target {
            Target::Window { surface, config } => {
                config.width = width;
                config.height = height;
                surface.surface().configure(&self.device, config);
            }
            Target::Texture(texture) => {
                *texture = Some(self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("picture"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                }));
            }
        }
    }

    fn forget_glyphs(&mut self) {
        self.atlas.clear();
    }

    fn paint(&mut self, frame: &Frame, glyphs: &mut dyn GlyphSource) -> Option<Rect> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let same = !frame.full && frame.changed.is_empty() && frame.cursor == self.drawn_cursor;
        if same && !self.lost {
            return None;
        }
        // When the atlas is full it is emptied and the frame built again
        // with only the glyphs that the frame itself has. A frame with
        // more glyphs than the atlas holds is drawn without some.
        let floats = match self.build(frame, glyphs) {
            Ok(floats) => floats,
            Err(Full) => {
                self.atlas.clear();
                self.pictures.clear();
                self.build(frame, glyphs).unwrap_or_default()
            }
        };

        match &self.target {
            Target::Texture(Some(texture)) => {
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.draw(&view, frame, &floats);
            }
            Target::Texture(None) => return None,
            Target::Window { surface, config } => {
                let texture = match surface.surface().get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(texture)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
                    // The window is not what the surface was set up for:
                    // set it up again, and draw when next asked.
                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        surface.surface().configure(&self.device, config);
                        self.lost = true;
                        return None;
                    }
                    _ => {
                        self.lost = true;
                        return None;
                    }
                };
                let view = texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                self.draw(&view, frame, &floats);
                self.queue.present(texture);
            }
        }
        self.drawn_cursor = frame.cursor;
        self.lost = false;
        Some(Rect {
            x: 0,
            y: 0,
            w: self.width,
            h: self.height,
        })
    }
}
