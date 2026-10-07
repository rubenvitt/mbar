//! Graphics: windows, Metal renderer, CoreText, images.
//!
//! Crate-local, renderer-facing API; `platform` maps `mbar_core::scene` onto it.
//!
//! * [`scene`]: [`DrawList`](scene::DrawList) / [`DrawCmd`](scene::DrawCmd) in logical
//!   points, top-left origin, premultiplied colours.
//! * [`text`]: CoreText font resolution, measurement with an LRU layout cache, run
//!   rasterization into A8 / BGRA atlases.
//! * [`image`]: files (ImageIO), app icons (NSWorkspace) and `CGImage`s → textures.
//! * [`renderer`]: the shared Metal device, pipelines and frame encoding.
//! * [`window`]: borderless non-activating panels hosting a `CAMetalLayer`, levels,
//!   sticky, blur, mouse forwarding.
//!
//! Typical setup (main thread):
//!
//! ```ignore
//! let mut renderer = Renderer::new()?;
//! let mut text = TextSystem::new(renderer.device());
//! let mut images = ImageStore::new(renderer.device(), mtm);
//! let mut win = BarWindow::new(mtm, &renderer, frame);
//! win.show();
//! let font = text.font("Hack Nerd Font", "Bold", 14.0, None);
//! let l = text.measure(font, "hello", false);
//! list.push(DrawCmd::Text { origin, run: l.run, color, clip: None });
//! win.render(&list, &mut renderer, &mut text, &images);
//! ```

pub mod atlas;
mod gpu;
pub mod image;
pub mod renderer;
pub mod scene;
pub mod shaders;
pub mod text;
pub mod util;
pub mod window;

pub use image::{ImageInfo, ImageStore};
pub use renderer::{FrameStats, Renderer, RendererError};
pub use scene::{DrawCmd, DrawList, ImageId, Point, Rect, Rgba, TextRunId};
pub use text::{FontId, TextLayout, TextSystem};
pub use window::{
    level, BarWindow, Modifiers, MouseButton, MouseEvent, MouseEventKind, MouseHandler,
};
