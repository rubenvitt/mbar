//! Metal renderer shared by all bar/popup windows.
//!
//! One `MTLDevice` + command queue for the process. A frame of one window is a single
//! render pass into the window's `CAMetalLayer` drawable:
//!
//! * quads (rounded rects, text runs, images) are drawn as instanced triangle strips with
//!   one pipeline; consecutive quads are batched into one draw call as long as they use
//!   the same texture (untextured rects join any batch);
//! * graph paths are tessellated on the CPU into a triangle list and drawn with a second
//!   pipeline (per-path clip parameters).
//!
//! Clipping is evaluated per instance (axis-aligned rect + one rounded rect as an SDF),
//! so clip changes never break batches and need no scissor/stencil state.
//!
//! Triple buffering: a dispatch semaphore with three slots guards three per-frame vertex
//! buffers; the completion handler (one reusable block) signals it. Scratch vectors are
//! reused, so a steady-state frame performs no heap allocation in Rust code.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use block2::RcBlock;
use dispatch2::{DispatchRetained, DispatchSemaphore, DispatchTime};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLDrawable,
    MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState,
    MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter,
    MTLSamplerState, MTLStoreAction, MTLTexture,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use super::gpu::{self, Device, Texture};
use super::image::ImageStore;
use super::scene::{DrawCmd, DrawList, ImageId, Rect};
use super::shaders::{self, FLAG_NEAREST, KIND_COLOR_TEXT, KIND_IMAGE, KIND_MASK, KIND_RECT};
use super::text::TextSystem;
use super::util::{self, PathVertex};

/// Frames that may be in flight at once.
pub const MAX_FRAMES_IN_FLIGHT: usize = 3;

/// Pixel format of every window drawable.
pub const PIXEL_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// GPU instance of one quad; layout matches `QuadInstance` in `shaders::SOURCE`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct QuadInstance {
    rect: [f32; 4],
    color: [f32; 4],
    border_color: [f32; 4],
    uv: [f32; 4],
    clip: [f32; 4],
    rclip: [f32; 4],
    radius: f32,
    border_width: f32,
    rclip_radius: f32,
    kind: u32,
}

const _: () = assert!(size_of::<QuadInstance>() == 112);
const _: () = assert!(std::mem::offset_of!(QuadInstance, rclip) == 80);
const _: () = assert!(std::mem::offset_of!(QuadInstance, kind) == 108);
const _: () = assert!(size_of::<PathVertex>() == 32);
const _: () = assert!(std::mem::offset_of!(PathVertex, color) == 16);

/// `Uniforms` in the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Uniforms {
    viewport: [f32; 2],
    scale: f32,
    _pad: f32,
}

/// `ClipParams` in the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipParams {
    clip: [f32; 4],
    rclip: [f32; 4],
    rclip_radius: f32,
    scale: f32,
    _pad: [f32; 2],
}

const _: () = assert!(size_of::<ClipParams>() == 48);

#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipState {
    /// x0, y0, x1, y1
    rect: [f32; 4],
    /// x, y, w, h
    rclip: [f32; 4],
    /// < 0: none
    rclip_radius: f32,
}

const NO_CLIP: ClipState = ClipState {
    rect: [-1.0e7, -1.0e7, 1.0e7, 1.0e7],
    rclip: [0.0; 4],
    rclip_radius: -1.0,
};

impl ClipState {
    fn is_empty(&self) -> bool {
        !(self.rect[2] > self.rect[0] && self.rect[3] > self.rect[1])
    }

    fn intersects(&self, r: &Rect) -> bool {
        r.x < self.rect[2]
            && r.max_x() > self.rect[0]
            && r.y < self.rect[3]
            && r.max_y() > self.rect[1]
    }

    fn with_rect(&self, r: &Rect) -> ClipState {
        let x0 = self.rect[0].max(r.x);
        let y0 = self.rect[1].max(r.y);
        let x1 = self.rect[2].min(r.max_x()).max(x0);
        let y1 = self.rect[3].min(r.max_y()).max(y0);
        ClipState {
            rect: [x0, y0, x1, y1],
            ..*self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TexSource {
    None,
    Text { color: bool, page: u32 },
    Image(ImageId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Batch {
    Quads {
        tex: TexSource,
        start: u32,
        count: u32,
        /// Destination-out blending ([`DrawCmd::Erase`]).
        erase: bool,
    },
    Path {
        start: u32,
        count: u32,
        clip: ClipParams,
    },
}

struct FrameSlot {
    buffer: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
    capacity: usize,
}

/// Counters of the last encoded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameStats {
    pub quads: u32,
    pub path_vertices: u32,
    pub draw_calls: u32,
}

/// Error creating the renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RendererError {
    NoDevice,
    NoQueue,
    Shader(String),
    Pipeline(String),
    Resource(&'static str),
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoDevice => write!(f, "no Metal device"),
            RendererError::NoQueue => write!(f, "cannot create a Metal command queue"),
            RendererError::Shader(e) => write!(f, "shader compilation failed: {e}"),
            RendererError::Pipeline(e) => write!(f, "pipeline creation failed: {e}"),
            RendererError::Resource(what) => write!(f, "cannot create {what}"),
        }
    }
}

impl std::error::Error for RendererError {}

type CompletionBlock = RcBlock<dyn Fn(NonNull<ProtocolObject<dyn MTLCommandBuffer>>)>;

/// The Metal renderer (main thread only).
pub struct Renderer {
    device: Device,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    quad_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Quad pipeline with destination-out blending (`dst *= 1 - src.a`).
    erase_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    path_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    nearest: Retained<ProtocolObject<dyn MTLSamplerState>>,
    linear: Retained<ProtocolObject<dyn MTLSamplerState>>,
    dummy: Texture,
    frames: [FrameSlot; MAX_FRAMES_IN_FLIGHT],
    frame_index: usize,
    semaphore: DispatchRetained<DispatchSemaphore>,
    completion: CompletionBlock,
    submitted: u64,
    completed: Arc<AtomicU64>,
    quads: Vec<QuadInstance>,
    path_vertices: Vec<PathVertex>,
    batches: Vec<Batch>,
    clips: Vec<ClipState>,
    stats: FrameStats,
}

fn make_pipeline(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    vertex: &str,
    fragment: &str,
    erase: bool,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, RendererError> {
    let vf = library
        .newFunctionWithName(&NSString::from_str(vertex))
        .ok_or_else(|| RendererError::Shader(format!("missing function {vertex}")))?;
    let ff = library
        .newFunctionWithName(&NSString::from_str(fragment))
        .ok_or_else(|| RendererError::Shader(format!("missing function {fragment}")))?;
    let desc = MTLRenderPipelineDescriptor::new();
    desc.setVertexFunction(Some(&vf));
    desc.setFragmentFunction(Some(&ff));
    // SAFETY: index 0 is a valid colour attachment index.
    let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    att.setPixelFormat(PIXEL_FORMAT);
    att.setBlendingEnabled(true);
    att.setRgbBlendOperation(MTLBlendOperation::Add);
    att.setAlphaBlendOperation(MTLBlendOperation::Add);
    // Premultiplied "over"; the erase pipeline ignores the source colour and scales the
    // destination by `1 - src.a` (CoreGraphics' `kCGBlendModeDestinationOut`).
    let src = if erase {
        MTLBlendFactor::Zero
    } else {
        MTLBlendFactor::One
    };
    att.setSourceRGBBlendFactor(src);
    att.setSourceAlphaBlendFactor(src);
    att.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    att.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    device
        .newRenderPipelineStateWithDescriptor_error(&desc)
        .map_err(|e| RendererError::Pipeline(e.localizedDescription().to_string()))
}

fn make_sampler(
    device: &ProtocolObject<dyn MTLDevice>,
    filter: MTLSamplerMinMagFilter,
) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>, RendererError> {
    let desc = MTLSamplerDescriptor::new();
    desc.setMinFilter(filter);
    desc.setMagFilter(filter);
    desc.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
    desc.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
    device
        .newSamplerStateWithDescriptor(&desc)
        .ok_or(RendererError::Resource("sampler"))
}

fn rect4(r: &Rect) -> [f32; 4] {
    [r.x, r.y, r.width, r.height]
}

impl Renderer {
    /// Create the device, queue, pipelines (shaders compiled from source) and samplers.
    pub fn new() -> Result<Self, RendererError> {
        let device = MTLCreateSystemDefaultDevice().ok_or(RendererError::NoDevice)?;
        let queue = device.newCommandQueue().ok_or(RendererError::NoQueue)?;
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(shaders::SOURCE), None)
            .map_err(|e| RendererError::Shader(e.localizedDescription().to_string()))?;
        let quad_pipeline =
            make_pipeline(&device, &library, "quad_vertex", "quad_fragment", false)?;
        let erase_pipeline =
            make_pipeline(&device, &library, "quad_vertex", "quad_fragment", true)?;
        let path_pipeline =
            make_pipeline(&device, &library, "path_vertex", "path_fragment", false)?;
        let nearest = make_sampler(&device, MTLSamplerMinMagFilter::Nearest)?;
        let linear = make_sampler(&device, MTLSamplerMinMagFilter::Linear)?;
        let dummy = gpu::new_texture(&device, MTLPixelFormat::R8Unorm, 1, 1)
            .ok_or(RendererError::Resource("dummy texture"))?;
        gpu::upload(&dummy, 0, 0, 1, 1, &[0u8], 1, 1);

        let semaphore = DispatchSemaphore::new(MAX_FRAMES_IN_FLIGHT as isize);
        let completed = Arc::new(AtomicU64::new(0));
        let completion: CompletionBlock = {
            let semaphore = semaphore.clone();
            let completed = completed.clone();
            RcBlock::new(move |_cb: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                // Command buffers of one queue complete in submission order, so the count
                // of completed buffers is the serial of the newest completed frame.
                completed.fetch_add(1, Ordering::Release);
                semaphore.signal();
            })
        };
        Ok(Renderer {
            device,
            queue,
            quad_pipeline,
            erase_pipeline,
            path_pipeline,
            nearest,
            linear,
            dummy,
            frames: std::array::from_fn(|_| FrameSlot {
                buffer: None,
                capacity: 0,
            }),
            frame_index: 0,
            semaphore,
            completion,
            submitted: 0,
            completed,
            quads: Vec::with_capacity(256),
            path_vertices: Vec::with_capacity(1024),
            batches: Vec::with_capacity(64),
            clips: Vec::with_capacity(8),
            stats: FrameStats::default(),
        })
    }

    /// The shared device (pass it to [`TextSystem::new`] and [`ImageStore::new`]).
    pub fn device(&self) -> &ProtocolObject<dyn MTLDevice> {
        &self.device
    }

    /// Frames committed so far.
    pub fn submitted_frames(&self) -> u64 {
        self.submitted
    }

    /// Frames the GPU has finished.
    pub fn completed_frames(&self) -> u64 {
        self.completed.load(Ordering::Acquire)
    }

    /// Counters of the most recent frame.
    pub fn last_frame_stats(&self) -> FrameStats {
        self.stats
    }

    /// Block until every submitted frame has completed (e.g. before tearing down).
    pub fn wait_idle(&self) {
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            self.semaphore.wait(DispatchTime::FOREVER);
        }
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            self.semaphore.signal();
        }
    }

    /// Configure a layer for this renderer (device, pixel format, transparency).
    pub(crate) fn configure_layer(&self, layer: &CAMetalLayer) {
        layer.setDevice(Some(&self.device));
        layer.setPixelFormat(PIXEL_FORMAT);
        layer.setFramebufferOnly(true);
        layer.setOpaque(false);
        layer.setMaximumDrawableCount(MAX_FRAMES_IN_FLIGHT);
    }

    /// Render `list` into the next drawable of `layer`. `size` is the layer size in
    /// points, `scale` its contents scale. Returns `false` if nothing was presented
    /// (no drawable available).
    pub(crate) fn render(
        &mut self,
        layer: &CAMetalLayer,
        list: &DrawList,
        size: (f32, f32),
        scale: f32,
        text: &mut TextSystem,
        images: &ImageStore,
    ) -> bool {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.build(list, scale, text, images);
        // Wait for a free frame slot (the oldest in-flight frame to finish).
        self.semaphore.wait(DispatchTime::FOREVER);
        let presented = autoreleasepool(|_| self.encode(layer, size, scale, text, images));
        if !presented {
            self.semaphore.signal();
        }
        presented
    }

    // -----------------------------------------------------------------------------------
    // CPU side: DrawList → instances / vertices / batches
    // -----------------------------------------------------------------------------------

    fn build(&mut self, list: &DrawList, scale: f32, text: &mut TextSystem, images: &ImageStore) {
        self.quads.clear();
        self.path_vertices.clear();
        self.batches.clear();
        self.clips.clear();
        let serial = self.submitted + 1;
        let completed = self.completed_frames();
        let mut clip = NO_CLIP;
        let aa = 1.0 / scale;

        for cmd in &list.items {
            match cmd {
                DrawCmd::PushClip {
                    rect,
                    corner_radius,
                } => {
                    self.clips.push(clip);
                    let mut next = clip.with_rect(rect);
                    if *corner_radius > 0.0 {
                        next.rclip = rect4(rect);
                        next.rclip_radius = corner_radius
                            .min(rect.width / 2.0)
                            .min(rect.height / 2.0)
                            .max(0.0);
                    }
                    clip = next;
                }
                DrawCmd::PopClip => {
                    if let Some(c) = self.clips.pop() {
                        clip = c;
                    }
                }
                _ if clip.is_empty() => {}
                DrawCmd::RoundedRect {
                    rect,
                    fill,
                    corner_radius,
                    border_width,
                    border_color,
                } => {
                    let bw = border_width.max(0.0);
                    let (inset, r) = util::rounded_rect_geometry(*rect, *corner_radius, bw);
                    if inset.is_empty() || !clip.intersects(rect) {
                        continue;
                    }
                    let has_border = bw > 0.0 && border_color[3] > 0.0;
                    if fill[3] <= 0.0 && !has_border {
                        continue;
                    }
                    self.push_quad(
                        QuadInstance {
                            rect: rect4(rect),
                            color: *fill,
                            border_color: *border_color,
                            uv: [0.0; 4],
                            clip: clip.rect,
                            rclip: clip.rclip,
                            radius: r,
                            border_width: bw,
                            rclip_radius: clip.rclip_radius,
                            kind: KIND_RECT,
                        },
                        TexSource::None,
                        false,
                    );
                }
                DrawCmd::Erase {
                    rect,
                    corner_radius,
                    alpha,
                    stroke_width,
                    stroke_alpha,
                } => {
                    let sw = stroke_width.max(0.0);
                    let a = alpha.clamp(0.0, 1.0);
                    let sa = if sw > 0.0 {
                        stroke_alpha.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    if rect.is_empty() || (a <= 0.0 && sa <= 0.0) {
                        continue;
                    }
                    // Grow the region by half the stroke so that the rounded-rect shader's
                    // inset (`border_width / 2`) lands exactly on `rect`: fill = `rect`,
                    // stroke centred on its edge.
                    let region = rect.inset(-sw / 2.0, -sw / 2.0);
                    if !clip.intersects(&region) {
                        continue;
                    }
                    let r = util::clamp_corner_radius(rect.width, rect.height, *corner_radius);
                    self.push_quad(
                        QuadInstance {
                            rect: rect4(&region),
                            color: [0.0, 0.0, 0.0, a],
                            border_color: [0.0, 0.0, 0.0, sa],
                            uv: [0.0; 4],
                            clip: clip.rect,
                            rclip: clip.rclip,
                            radius: r,
                            border_width: sw,
                            rclip_radius: clip.rclip_radius,
                            kind: KIND_RECT,
                        },
                        TexSource::None,
                        true,
                    );
                }
                DrawCmd::Text {
                    origin,
                    run,
                    color,
                    clip: text_clip,
                } => {
                    let c = match text_clip {
                        Some(r) => clip.with_rect(r),
                        None => clip,
                    };
                    if c.is_empty() {
                        continue;
                    }
                    let Some(g) = text.glyph_quad(*run, scale, *color, serial, completed) else {
                        continue;
                    };
                    if !g.color && color[3] <= 0.0 {
                        continue;
                    }
                    let ox = util::snap(origin.x, scale);
                    let oy = util::snap(origin.y, scale);
                    let r = Rect::new(ox + g.dx, oy + g.dy, g.w, g.h);
                    if !c.intersects(&r) {
                        continue;
                    }
                    let kind = if g.color { KIND_COLOR_TEXT } else { KIND_MASK } | FLAG_NEAREST;
                    self.push_quad(
                        QuadInstance {
                            rect: rect4(&r),
                            color: *color,
                            border_color: [0.0; 4],
                            uv: g.uv,
                            clip: c.rect,
                            rclip: c.rclip,
                            radius: 0.0,
                            border_width: 0.0,
                            rclip_radius: c.rclip_radius,
                            kind,
                        },
                        TexSource::Text {
                            color: g.color,
                            page: g.page,
                        },
                        false,
                    );
                }
                DrawCmd::Image {
                    rect,
                    image,
                    corner_radius,
                    border_width,
                    border_color,
                    tint,
                    nearest,
                } => {
                    if rect.is_empty() || !clip.intersects(rect) || images.texture(*image).is_none()
                    {
                        continue;
                    }
                    let rounded = util::image_is_rounded(rect.width, rect.height, *corner_radius);
                    let (radius, bw) = if rounded {
                        (corner_radius.max(0.0), border_width.max(0.0))
                    } else {
                        (0.0, 0.0)
                    };
                    let kind = KIND_IMAGE | if *nearest { FLAG_NEAREST } else { 0 };
                    self.push_quad(
                        QuadInstance {
                            rect: rect4(rect),
                            color: *tint,
                            border_color: *border_color,
                            uv: [0.0, 0.0, 1.0, 1.0],
                            clip: clip.rect,
                            rclip: clip.rclip,
                            radius,
                            border_width: if border_color[3] > 0.0 { bw } else { 0.0 },
                            rclip_radius: clip.rclip_radius,
                            kind,
                        },
                        TexSource::Image(*image),
                        false,
                    );
                }
                DrawCmd::Path {
                    points,
                    baseline,
                    line_color,
                    fill_color,
                    line_width,
                    fill,
                } => {
                    let pts = list.path_points(points);
                    let start = self.path_vertices.len();
                    util::tessellate_stroke(
                        pts,
                        *line_width,
                        aa,
                        *line_color,
                        &mut self.path_vertices,
                    );
                    if *fill {
                        util::tessellate_fill(pts, *baseline, *fill_color, &mut self.path_vertices);
                    }
                    let count = self.path_vertices.len() - start;
                    if count == 0 {
                        continue;
                    }
                    self.batches.push(Batch::Path {
                        start: start as u32,
                        count: count as u32,
                        clip: ClipParams {
                            clip: clip.rect,
                            rclip: clip.rclip,
                            rclip_radius: clip.rclip_radius,
                            scale,
                            _pad: [0.0; 2],
                        },
                    });
                }
            }
        }
    }

    fn push_quad(&mut self, q: QuadInstance, tex: TexSource, erase: bool) {
        let index = self.quads.len() as u32;
        self.quads.push(q);
        if let Some(Batch::Quads {
            tex: batch_tex,
            count,
            erase: batch_erase,
            ..
        }) = self.batches.last_mut()
        {
            let compatible = *batch_erase == erase
                && (tex == TexSource::None || *batch_tex == TexSource::None || *batch_tex == tex);
            if compatible {
                if *batch_tex == TexSource::None {
                    *batch_tex = tex;
                }
                *count += 1;
                return;
            }
        }
        self.batches.push(Batch::Quads {
            tex,
            start: index,
            count: 1,
            erase,
        });
    }

    // -----------------------------------------------------------------------------------
    // GPU side
    // -----------------------------------------------------------------------------------

    /// Copy this frame's data into the current slot buffer. Returns the offset of the
    /// path vertices.
    fn upload_frame_data(&mut self) -> Option<usize> {
        let quad_bytes = self.quads.len() * size_of::<QuadInstance>();
        let path_offset = quad_bytes.div_ceil(256) * 256;
        let path_bytes = self.path_vertices.len() * size_of::<PathVertex>();
        let needed = path_offset + path_bytes;
        if needed == 0 {
            return Some(0);
        }
        let slot = &mut self.frames[self.frame_index];
        if slot.capacity < needed || slot.buffer.is_none() {
            let capacity = needed.next_power_of_two().max(64 * 1024);
            slot.buffer = self.device.newBufferWithLength_options(
                capacity,
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined,
            );
            slot.capacity = if slot.buffer.is_some() { capacity } else { 0 };
        }
        let buffer = slot.buffer.as_ref()?;
        let dst = buffer.contents().as_ptr() as *mut u8;
        // SAFETY: the buffer is `capacity >= needed` bytes of CPU-visible shared memory
        // that the GPU is not reading (this slot's previous frame completed: we hold the
        // semaphore). Source slices are plain-old-data `#[repr(C)]` values, and the
        // destination ranges `[0, quad_bytes)` and `[path_offset, needed)` do not overlap
        // the sources.
        unsafe {
            std::ptr::copy_nonoverlapping(self.quads.as_ptr() as *const u8, dst, quad_bytes);
            std::ptr::copy_nonoverlapping(
                self.path_vertices.as_ptr() as *const u8,
                dst.add(path_offset),
                path_bytes,
            );
        }
        Some(path_offset)
    }

    fn encode(
        &mut self,
        layer: &CAMetalLayer,
        size: (f32, f32),
        scale: f32,
        text: &TextSystem,
        images: &ImageStore,
    ) -> bool {
        let Some(path_offset) = self.upload_frame_data() else {
            return false;
        };
        let Some(drawable) = layer.nextDrawable() else {
            return false;
        };
        let Some(cb) = self.queue.commandBuffer() else {
            return false;
        };
        let target = drawable.texture();
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 is a valid colour attachment index.
        let att = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        att.setTexture(Some(&target));
        att.setLoadAction(MTLLoadAction::Clear);
        att.setStoreAction(MTLStoreAction::Store);
        att.setClearColor(MTLClearColor {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.0,
        });
        let Some(enc) = cb.renderCommandEncoderWithDescriptor(&pass) else {
            return false;
        };

        let uniforms = Uniforms {
            viewport: [size.0.max(1.0), size.1.max(1.0)],
            scale,
            _pad: 0.0,
        };
        let uniforms_ptr = NonNull::from(&uniforms).cast::<c_void>();
        let buffer = self.frames[self.frame_index].buffer.clone();
        let mut draw_calls = 0u32;
        // 0 = none, 1 = quads, 2 = paths, 3 = erase quads
        let mut bound = 0u8;

        for batch in &self.batches {
            match *batch {
                Batch::Quads {
                    tex,
                    start,
                    count,
                    erase,
                } => {
                    let Some(buf) = buffer.as_deref() else {
                        continue;
                    };
                    let want = if erase { 3 } else { 1 };
                    if bound == 1 || bound == 3 {
                        if bound != want {
                            enc.setRenderPipelineState(if erase {
                                &self.erase_pipeline
                            } else {
                                &self.quad_pipeline
                            });
                            bound = want;
                        }
                    } else {
                        enc.setRenderPipelineState(if erase {
                            &self.erase_pipeline
                        } else {
                            &self.quad_pipeline
                        });
                        // SAFETY: the buffer outlives the command buffer (Metal retains
                        // it); the uniform bytes are copied by Metal during the call and
                        // match the shader's `Uniforms` layout; buffer/sampler indices
                        // match the shader's argument table.
                        unsafe {
                            enc.setVertexBuffer_offset_atIndex(Some(buf), 0, 0);
                            enc.setVertexBytes_length_atIndex(
                                uniforms_ptr,
                                size_of::<Uniforms>(),
                                1,
                            );
                            enc.setFragmentBuffer_offset_atIndex(Some(buf), 0, 0);
                            enc.setFragmentBytes_length_atIndex(
                                uniforms_ptr,
                                size_of::<Uniforms>(),
                                1,
                            );
                            enc.setFragmentSamplerState_atIndex(Some(&self.nearest), 0);
                            enc.setFragmentSamplerState_atIndex(Some(&self.linear), 1);
                        }
                        bound = want;
                    }
                    let texture: &ProtocolObject<dyn MTLTexture> = match tex {
                        TexSource::None => &self.dummy,
                        TexSource::Text { color, page } => {
                            text.page_texture(color, page).unwrap_or(&self.dummy)
                        }
                        TexSource::Image(id) => images.texture(id).unwrap_or(&self.dummy),
                    };
                    // SAFETY: valid texture at index 0; the draw reads `count` instances
                    // starting at `start`, all written to the bound buffer above.
                    unsafe {
                        enc.setFragmentTexture_atIndex(Some(texture), 0);
                        enc.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                            MTLPrimitiveType::TriangleStrip,
                            0,
                            4,
                            count as usize,
                            start as usize,
                        );
                    }
                    draw_calls += 1;
                }
                Batch::Path { start, count, clip } => {
                    let Some(buf) = buffer.as_deref() else {
                        continue;
                    };
                    if bound != 2 {
                        enc.setRenderPipelineState(&self.path_pipeline);
                        // SAFETY: as above; `path_offset` is 256-byte aligned and the
                        // path vertices were written there.
                        unsafe {
                            enc.setVertexBuffer_offset_atIndex(Some(buf), path_offset, 0);
                            enc.setVertexBytes_length_atIndex(
                                uniforms_ptr,
                                size_of::<Uniforms>(),
                                1,
                            );
                        }
                        bound = 2;
                    }
                    let clip_ptr = NonNull::from(&clip).cast::<c_void>();
                    // SAFETY: `ClipParams` matches the shader layout and is copied during
                    // the call; the vertex range lies inside the uploaded vertices.
                    unsafe {
                        enc.setFragmentBytes_length_atIndex(clip_ptr, size_of::<ClipParams>(), 0);
                        enc.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::Triangle,
                            start as usize,
                            count as usize,
                        );
                    }
                    draw_calls += 1;
                }
            }
        }
        enc.endEncoding();
        let drawable_ref: &ProtocolObject<dyn CAMetalDrawable> = &drawable;
        cb.presentDrawable(ProtocolObject::<dyn MTLDrawable>::from_ref(drawable_ref));
        // SAFETY: the completion block is a valid heap block that lives as long as the
        // renderer; Metal copies (retains) it. It only touches thread-safe state.
        unsafe { cb.addCompletedHandler(RcBlock::as_ptr(&self.completion)) };
        cb.commit();

        self.submitted += 1;
        self.frame_index = (self.frame_index + 1) % MAX_FRAMES_IN_FLIGHT;
        self.stats = FrameStats {
            quads: self.quads.len() as u32,
            path_vertices: self.path_vertices.len() as u32,
            draw_calls,
        };
        true
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // In-flight command buffers reference our semaphore/block; let them finish.
        self.wait_idle();
    }
}
