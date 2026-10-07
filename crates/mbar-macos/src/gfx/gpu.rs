//! Small Metal helpers shared by the text atlas, the image store and the renderer.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize, MTLStorageMode, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
};

pub type Device = Retained<ProtocolObject<dyn MTLDevice>>;
pub type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

/// Storage mode for CPU-written textures: `Shared` on unified memory (Apple silicon),
/// `Managed` otherwise (Intel Macs do not allow shared textures).
pub fn cpu_texture_storage(device: &ProtocolObject<dyn MTLDevice>) -> MTLStorageMode {
    if device.hasUnifiedMemory() {
        MTLStorageMode::Shared
    } else {
        MTLStorageMode::Managed
    }
}

/// Create a sampled 2D texture without mipmaps that the CPU fills via `replaceRegion`.
pub fn new_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
    width: u32,
    height: u32,
) -> Option<Texture> {
    if width == 0 || height == 0 {
        return None;
    }
    // SAFETY: plain descriptor factory; the sizes are non-zero and the format is a
    // valid colour format.
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format,
            width as usize,
            height as usize,
            false,
        )
    };
    desc.setUsage(MTLTextureUsage::ShaderRead);
    desc.setStorageMode(cpu_texture_storage(device));
    device.newTextureWithDescriptor(&desc)
}

/// Copy `bytes` (rows of `bytes_per_row`, `bytes_per_pixel` per texel) into the `w × h`
/// region at `(x, y)`.
///
/// Returns `false` (and does nothing) when `bytes` is too short for the region.
#[allow(clippy::too_many_arguments)]
pub fn upload(
    texture: &ProtocolObject<dyn MTLTexture>,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    bytes: &[u8],
    bytes_per_row: usize,
    bytes_per_pixel: usize,
) -> bool {
    if w == 0 || h == 0 {
        return true;
    }
    let row = w as usize * bytes_per_pixel;
    let needed = bytes_per_row * (h as usize - 1) + row;
    if bytes_per_row < row
        || bytes.len() < needed
        || x + w > texture.width() as u32
        || y + h > texture.height() as u32
    {
        return false;
    }
    let region = MTLRegion {
        origin: MTLOrigin {
            x: x as usize,
            y: y as usize,
            z: 0,
        },
        size: MTLSize {
            width: w as usize,
            height: h as usize,
            depth: 1,
        },
    };
    let ptr = NonNull::new(bytes.as_ptr() as *mut c_void).expect("slice pointer is non-null");
    // SAFETY: the region lies inside the texture and `bytes` holds at least
    // `bytes_per_row * (h - 1) + w * bytes_per_pixel` bytes (both checked above), which is
    // exactly what `replaceRegion` reads for this region. It only reads from the pointer.
    unsafe {
        texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(region, 0, ptr, bytes_per_row)
    };
    true
}
