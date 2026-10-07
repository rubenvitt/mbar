//! Pure mappings between `mbar-core` and the gfx / sys layers (no Apple calls), unit tested
//! on macOS CI:
//!
//! * [`scene_to_drawlist`]: `mbar_core::scene::Scene` → [`DrawList`].
//! * [`sys_event_to_input`]: stateless [`SysEvent`] → [`Input`] mapping.
//! * [`monitor_mouse_input`] / [`window_mouse_input`]: mouse events → [`MouseInput`].
//! * [`text_metrics`]: [`TextLayout`] → core [`TextMetrics`].

use crate::gfx::scene::{premultiply, DrawCmd, DrawList, ImageId, Point as GPoint, Rect as GRect};
use crate::gfx::scene::{Rgba, TextRunId, TRANSPARENT};
use crate::gfx::text::TextLayout;
use crate::gfx::window::{MouseButton as WinButton, MouseEvent as WinMouse, MouseEventKind};
use crate::sys::mouse::{MouseEvent as SysMouse, MouseKind as SysMouseKind};
use crate::sys::SysEvent;
use mbar_core::color::Color;
use mbar_core::geometry::{Point, Rect};
use mbar_core::item::ItemId;
use mbar_core::platform::{
    ImageKey, Input, MouseButton, MouseInput, MouseKind, OsEvent, PowerSource, TextKey,
    TextMetrics, WindowKey,
};
use mbar_core::scene::{Primitive, Scene};

// ---------------------------------------------------------------------------------------
// Scene → DrawList
// ---------------------------------------------------------------------------------------

/// What [`scene_to_drawlist`] needs to resolve the core's opaque keys.
pub trait SceneLookup {
    /// The (possibly re-measured) run of a text line; `None` skips the text.
    fn text_run(&mut self, key: TextKey) -> Option<TextRunId>;
    /// The stored image of `key`; `None` skips the image.
    fn image(&self, key: ImageKey) -> Option<ImageId>;
}

/// Core colour → premultiplied RGBA (channels clamped like `color.c`).
pub fn rgba(c: &Color) -> Rgba {
    premultiply(c.r, c.g, c.b, c.a)
}

fn grect(r: &Rect) -> GRect {
    GRect::new(r.x, r.y, r.width, r.height)
}

fn gpoint(p: &Point) -> GPoint {
    GPoint::new(p.x, p.y)
}

/// `ImageKey` ↔ [`ImageId`] (the key is the store id).
pub fn image_key(id: ImageId) -> ImageKey {
    ImageKey(id.0 as u64)
}

/// Inverse of [`image_key`]; `None` for keys that cannot be store ids.
pub fn image_id(key: ImageKey) -> Option<ImageId> {
    u32::try_from(key.0).ok().filter(|v| *v != 0).map(ImageId)
}

/// Fills `list` (cleared first, capacity kept) with the drawing commands of `scene`.
///
/// | Primitive | DrawCmd |
/// |---|---|
/// | `Rect` | `RoundedRect` (premultiplied fill/border) |
/// | `Shadow` | `RoundedRect` with the shadow colour as fill and border |
/// | `Text` | `Text` (run resolved through [`SceneLookup::text_run`]) |
/// | `Image` | `Image`, nearest-neighbour; not `rounded` → no clip, no border |
/// | `ImageMask` | `tint` of the directly preceding `Image` of the same picture/rect, else a tinted `Image` |
/// | `Graph` | `Path` (points = `line`, baseline = closing y of `fill`, fill iff visible) |
/// | `ClipHole` | `Erase` |
/// | `PushClip` / `PopClip` | `PushClip` / `PopClip` |
/// | `BlurRegion` | — (realised as child windows by the window manager, see [`blur_regions`]) |
pub fn scene_to_drawlist(scene: &Scene, list: &mut DrawList, look: &mut impl SceneLookup) {
    list.clear();
    for p in &scene.primitives {
        match p {
            Primitive::Rect {
                rect,
                color,
                corner_radius,
                border_width,
                border_color,
            } => {
                let fill = rgba(color);
                let border = rgba(border_color);
                if fill[3] <= 0.0 && (*border_width <= 0.0 || border[3] <= 0.0) {
                    continue;
                }
                list.push(DrawCmd::RoundedRect {
                    rect: grect(rect),
                    fill,
                    corner_radius: *corner_radius,
                    border_width: *border_width,
                    border_color: border,
                });
            }
            Primitive::Shadow {
                rect,
                color,
                corner_radius,
                border_width,
            } => {
                let c = rgba(color);
                if c[3] <= 0.0 {
                    continue;
                }
                list.push(DrawCmd::RoundedRect {
                    rect: grect(rect),
                    fill: c,
                    corner_radius: *corner_radius,
                    border_width: *border_width,
                    border_color: c,
                });
            }
            Primitive::Text {
                origin,
                key,
                color,
                clip,
            } => {
                let Some(run) = look.text_run(*key) else {
                    continue;
                };
                list.push(DrawCmd::Text {
                    origin: gpoint(origin),
                    run,
                    color: rgba(color),
                    clip: clip.as_ref().map(grect),
                });
            }
            Primitive::Image {
                rect,
                key,
                corner_radius,
                rounded,
                border_width,
                border_color,
            } => {
                let Some(image) = look.image(*key) else {
                    continue;
                };
                let (corner_radius, border_width) = if *rounded {
                    (*corner_radius, *border_width)
                } else {
                    (0.0, 0.0)
                };
                list.push(DrawCmd::Image {
                    rect: grect(rect),
                    image,
                    corner_radius,
                    border_width,
                    border_color: rgba(border_color),
                    tint: TRANSPARENT,
                    nearest: true,
                });
            }
            Primitive::ImageMask { rect, key, color } => {
                let Some(id) = look.image(*key) else {
                    continue;
                };
                let tint = rgba(color);
                let r = grect(rect);
                if let Some(DrawCmd::Image {
                    rect: prev_rect,
                    image,
                    tint: prev_tint,
                    ..
                }) = list.items.last_mut()
                {
                    if *image == id && *prev_rect == r {
                        *prev_tint = tint;
                        continue;
                    }
                }
                list.push(DrawCmd::Image {
                    rect: r,
                    image: id,
                    corner_radius: 0.0,
                    border_width: 0.0,
                    border_color: TRANSPARENT,
                    tint,
                    nearest: true,
                });
            }
            Primitive::Graph {
                line,
                fill,
                line_color,
                fill_color,
                line_width,
            } => {
                if line.is_empty() {
                    continue;
                }
                let fill_rgba = rgba(fill_color);
                // `fill` is `line` closed along the baseline: its last point lies on it.
                let baseline = fill
                    .last()
                    .map(|p| p.y)
                    .unwrap_or_else(|| line.iter().map(|p| p.y).fold(f32::MIN, f32::max));
                let has_fill = fill_rgba[3] > 0.0 && fill.len() >= line.len() + 2;
                list.push_path(
                    line.iter().map(gpoint),
                    baseline,
                    rgba(line_color),
                    fill_rgba,
                    *line_width,
                    has_fill,
                );
            }
            Primitive::ClipHole {
                rect,
                corner_radius,
                alpha,
                stroke_width,
                stroke_alpha,
            } => list.push(DrawCmd::Erase {
                rect: grect(rect),
                corner_radius: *corner_radius,
                alpha: *alpha,
                stroke_width: *stroke_width,
                stroke_alpha: *stroke_alpha,
            }),
            Primitive::PushClip {
                rect,
                corner_radius,
            } => list.push(DrawCmd::PushClip {
                rect: grect(rect),
                corner_radius: *corner_radius,
            }),
            Primitive::PopClip => list.push(DrawCmd::PopClip),
            Primitive::BlurRegion { .. } => {}
        }
    }
}

/// A blur region of a scene (window-local rect, radius).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlurSpec {
    pub rect: Rect,
    pub corner_radius: f32,
    pub radius: u32,
}

/// The `BlurRegion`s of `scene` in paint order (radius 0 / empty rects skipped).
pub fn blur_regions(scene: &Scene) -> impl Iterator<Item = BlurSpec> + '_ {
    scene.primitives.iter().filter_map(|p| match p {
        Primitive::BlurRegion {
            rect,
            corner_radius,
            radius,
        } if *radius > 0 && rect.width > 0.0 && rect.height > 0.0 => Some(BlurSpec {
            rect: *rect,
            corner_radius: *corner_radius,
            radius: *radius,
        }),
        _ => None,
    })
}

// ---------------------------------------------------------------------------------------
// Text metrics
// ---------------------------------------------------------------------------------------

/// [`TextLayout`] (already rounded by `text_prepare_line`) → core [`TextMetrics`].
///
/// The core applies SketchyBar's rounding itself (`(i32)(x+0.5)`, `(u32)(w+1.5)`,
/// `(u32)(tw+0.5)`), so the rounded integers are handed over shifted such that this
/// rounding reproduces them exactly. An empty string has an empty ink rect.
pub fn text_metrics(key: TextKey, empty: bool, l: &TextLayout) -> TextMetrics {
    let ink = if empty {
        Rect::ZERO
    } else {
        Rect::new(
            l.ink_x - 0.5,
            l.ink_y - 0.5,
            l.ink_width - 1.5,
            l.ink_height - 1.5,
        )
    };
    TextMetrics {
        key,
        ink,
        typographic_width: (l.typographic_width - 0.5).max(-0.5),
        ascent: l.ascent,
        descent: l.descent,
    }
}

// ---------------------------------------------------------------------------------------
// SysEvent → Input
// ---------------------------------------------------------------------------------------

/// `"AC"` / `"BATTERY"` → [`PowerSource`].
pub fn power_source(s: &str) -> Option<PowerSource> {
    match s {
        "AC" => Some(PowerSource::Ac),
        "BATTERY" => Some(PowerSource::Battery),
        _ => None,
    }
}

/// Stateless mapping of a system event. Returns `None` for events that need platform
/// state (media artwork, alias captures, mach messages) and for events with no core
/// counterpart (mouse-down). Mouse inputs carry `window: None`; the platform resolves it.
///
/// | SysEvent | Input |
/// |---|---|
/// | `FrontAppSwitched` | `Event(FrontAppSwitched{name, bundle_id})` |
/// | `SpaceChange` | `Event(SpaceChanged)` |
/// | `SpaceWindowsChange` | `Event(SpaceWindowsChanged(info))` |
/// | `DisplayChange` | `Event(ActiveDisplayChanged)` |
/// | `DisplaysReconfigured` | `DisplaysChanged` |
/// | `MenuBarHidingChanged` | `Event(MenuBarHiddenChanged)` |
/// | `VolumeChange` / `BrightnessChange` | `Event(VolumeChanged / BrightnessChanged)` |
/// | `PowerSourceChange` | `Event(PowerSourceChanged)` (unknown strings dropped) |
/// | `WifiChange` / `MediaChange` | `Event(WifiChanged / MediaChanged)` |
/// | `SystemWillSleep` / `SystemWoke` | `Event(SystemWillSleep / SystemWoke)` |
/// | `DistributedNotification` | `Event(DistributedNotification{name, info})` |
/// | `MenusChanged` | `MenuTitles{app, titles}` |
/// | `ProviderSample` | `ProviderSample{item: ItemId(id), values}` |
/// | `CaptureGating` | `Event(CaptureDisabled)` |
/// | `Mouse` | `Mouse` via [`monitor_mouse_input`] |
/// | `ConfigChanged` | `Event(ConfigChanged)` |
/// | `ScriptFinished` | `ScriptFinished{pid, item: None, output}` |
/// | `MediaArtwork`, `AliasUpdate`, `MachMessage` | `None` (stateful, see `Bridge`) |
pub fn sys_event_to_input(ev: SysEvent) -> Option<Input> {
    Some(match ev {
        SysEvent::FrontAppSwitched {
            name, bundle_id, ..
        } => Input::Event(OsEvent::FrontAppSwitched { name, bundle_id }),
        SysEvent::SpaceChange { .. } => Input::Event(OsEvent::SpaceChanged),
        SysEvent::SpaceWindowsChange { info_json, .. } => {
            Input::Event(OsEvent::SpaceWindowsChanged(info_json))
        }
        SysEvent::DisplayChange { .. } => Input::Event(OsEvent::ActiveDisplayChanged),
        SysEvent::DisplaysReconfigured { .. } => Input::DisplaysChanged,
        SysEvent::MenuBarHidingChanged => Input::Event(OsEvent::MenuBarHiddenChanged),
        SysEvent::VolumeChange(v) => Input::Event(OsEvent::VolumeChanged(v)),
        SysEvent::BrightnessChange(v) => Input::Event(OsEvent::BrightnessChanged(v)),
        SysEvent::PowerSourceChange(s) => Input::Event(OsEvent::PowerSourceChanged(
            power_source(&s)?,
        )),
        SysEvent::WifiChange(s) => Input::Event(OsEvent::WifiChanged(s)),
        SysEvent::MediaChange(s) => Input::Event(OsEvent::MediaChanged(s)),
        SysEvent::SystemWillSleep => Input::Event(OsEvent::SystemWillSleep),
        SysEvent::SystemWoke { .. } => Input::Event(OsEvent::SystemWoke),
        SysEvent::DistributedNotification {
            name,
            user_info_json,
        } => Input::Event(OsEvent::DistributedNotification {
            name,
            info: user_info_json,
        }),
        SysEvent::MenusChanged { app, titles, .. } => Input::MenuTitles { app, titles },
        SysEvent::ProviderSample { id, values, .. } => Input::ProviderSample {
            item: ItemId(id),
            values,
        },
        SysEvent::CaptureGating { disabled } => Input::Event(OsEvent::CaptureDisabled(disabled)),
        SysEvent::Mouse(m) => Input::Mouse(monitor_mouse_input(&m, None)?),
        SysEvent::ConfigChanged => Input::Event(OsEvent::ConfigChanged),
        SysEvent::ScriptFinished { pid, output, .. } => Input::ScriptFinished {
            pid,
            item: None,
            output,
        },
        SysEvent::MediaArtwork(_) | SysEvent::AliasUpdate { .. } | SysEvent::MachMessage { .. } => {
            return None
        }
    })
}

/// CG event type of a button release → [`MouseButton`] (`get_type_description`: 2 = left
/// up, 4 = right up, else other).
pub fn button_of_cg_type(cg_type: u32) -> MouseButton {
    match cg_type {
        2 => MouseButton::Left,
        4 => MouseButton::Right,
        _ => MouseButton::Other,
    }
}

/// An NSEvent-monitor mouse event (exact CG data) → [`MouseInput`]. Mouse-down has no core
/// counterpart (clicks fire on release).
pub fn monitor_mouse_input(m: &SysMouse, window: Option<WindowKey>) -> Option<MouseInput> {
    let kind = match m.kind {
        SysMouseKind::Down => return None,
        SysMouseKind::Up => MouseKind::Up {
            button: button_of_cg_type(m.cg_type),
            button_code: m.button.clamp(0, u32::MAX as i64) as u32,
        },
        SysMouseKind::Moved => MouseKind::Moved,
        SysMouseKind::Dragged => MouseKind::Dragged,
        SysMouseKind::Scrolled { delta, .. } => MouseKind::Scrolled { delta },
    };
    Some(MouseInput {
        kind,
        point: Point::new(m.location.x as f32, m.location.y as f32),
        window,
        modifiers: m.modifiers,
    })
}

/// Scroll delta of a view event in lines (fallback when the monitor did not see the
/// event): wheel deltas are rounded, precise (trackpad) pixel deltas divided by 10.
pub fn view_scroll_lines(dy: f32, precise: bool) -> i32 {
    let v = if precise { dy / 10.0 } else { dy };
    let r = v.round() as i32;
    if r == 0 && dy != 0.0 {
        dy.signum() as i32
    } else {
        r
    }
}

/// A bar/popup view event (window-local point) → [`MouseInput`] with the global point
/// `origin + location`. Mouse-down is dropped.
pub fn window_mouse_input(key: WindowKey, origin: Point, ev: &WinMouse) -> Option<MouseInput> {
    let kind = match ev.kind {
        MouseEventKind::Down => return None,
        MouseEventKind::Up => {
            let (button, button_code) = match ev.button {
                WinButton::Left | WinButton::None => (MouseButton::Left, 0),
                WinButton::Right => (MouseButton::Right, 1),
                WinButton::Other(n) => (MouseButton::Other, n.max(0) as u32),
            };
            MouseKind::Up {
                button,
                button_code,
            }
        }
        MouseEventKind::Dragged => MouseKind::Dragged,
        MouseEventKind::Moved => MouseKind::Moved,
        MouseEventKind::Entered => MouseKind::Entered,
        MouseEventKind::Exited => MouseKind::Exited,
        MouseEventKind::Scroll => MouseKind::Scrolled {
            delta: view_scroll_lines(ev.scroll_delta.1, ev.precise_scroll),
        },
    };
    Some(MouseInput {
        kind,
        point: Point::new(origin.x + ev.location.x, origin.y + ev.location.y),
        window: Some(key),
        // NSEventModifierFlags share the device-independent bits with CGEventFlags.
        modifiers: ev.modifiers.raw as u32,
    })
}

/// `true` for the view events that the local NSEvent monitor also reports with exact CG
/// data (releases and scrolls); see the click de-duplication in `Bridge`.
pub fn monitored_kind(kind: MouseEventKind) -> bool {
    matches!(kind, MouseEventKind::Up | MouseEventKind::Scroll)
}

/// Window level (`CGWindowLevel`) of the core → AppKit level.
pub fn window_level(level: i32) -> isize {
    level as isize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::window::Modifiers;
    use objc2_core_foundation::CGPoint;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake {
        runs: HashMap<u64, u64>,
        images: Vec<u64>,
    }

    impl SceneLookup for Fake {
        fn text_run(&mut self, key: TextKey) -> Option<TextRunId> {
            self.runs.get(&key.0).map(|r| TextRunId(*r))
        }
        fn image(&self, key: ImageKey) -> Option<ImageId> {
            self.images.contains(&key.0).then(|| image_id(key)).flatten()
        }
    }

    fn color(hex: u32) -> Color {
        Color::from_hex(hex)
    }

    #[test]
    fn rects_shadows_clips() {
        let mut s = Scene::default();
        s.push(Primitive::Shadow {
            rect: Rect::new(2.0, 2.0, 10.0, 10.0),
            color: color(0x80000000),
            corner_radius: 3.0,
            border_width: 1.0,
        });
        s.push(Primitive::PushClip {
            rect: Rect::new(0.0, 0.0, 50.0, 20.0),
            corner_radius: 0.0,
        });
        s.push(Primitive::Rect {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: color(0xff0000ff),
            corner_radius: 4.0,
            border_width: 2.0,
            border_color: color(0x80ffffff),
        });
        // Invisible: skipped.
        s.push(Primitive::Rect {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: color(0x00ff0000),
            corner_radius: 0.0,
            border_width: 0.0,
            border_color: color(0xffffffff),
        });
        s.push(Primitive::PopClip);
        let mut list = DrawList::new();
        scene_to_drawlist(&s, &mut list, &mut Fake::default());
        assert_eq!(list.items.len(), 4);
        let a = 128.0 / 255.0;
        assert_eq!(
            list.items[0],
            DrawCmd::RoundedRect {
                rect: GRect::new(2.0, 2.0, 10.0, 10.0),
                fill: [0.0, 0.0, 0.0, a],
                corner_radius: 3.0,
                border_width: 1.0,
                border_color: [0.0, 0.0, 0.0, a],
            }
        );
        assert_eq!(
            list.items[1],
            DrawCmd::PushClip {
                rect: GRect::new(0.0, 0.0, 50.0, 20.0),
                corner_radius: 0.0
            }
        );
        match &list.items[2] {
            DrawCmd::RoundedRect {
                fill,
                border_color,
                border_width,
                ..
            } => {
                assert_eq!(*fill, [0.0, 0.0, 1.0, 1.0]);
                assert!((border_color[0] - a).abs() < 1e-6 && (border_color[3] - a).abs() < 1e-6);
                assert_eq!(*border_width, 2.0);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(list.items[3], DrawCmd::PopClip);
    }

    #[test]
    fn text_and_images() {
        let mut fake = Fake::default();
        fake.runs.insert(7, 70);
        fake.images.push(3);
        let mut s = Scene::default();
        s.push(Primitive::Text {
            origin: Point::new(4.0, 20.0),
            key: TextKey(7),
            color: color(0xffffffff),
            clip: Some(Rect::new(0.0, -9999.0, 30.0, 19998.0)),
        });
        s.push(Primitive::Text {
            origin: Point::new(4.0, 20.0),
            key: TextKey(8), // unknown: skipped
            color: color(0xffffffff),
            clip: None,
        });
        s.push(Primitive::Image {
            rect: Rect::new(0.0, 0.0, 20.0, 20.0),
            key: ImageKey(3),
            corner_radius: 4.0,
            rounded: true,
            border_width: 1.0,
            border_color: color(0xffff0000),
        });
        s.push(Primitive::ImageMask {
            rect: Rect::new(0.0, 0.0, 20.0, 20.0),
            key: ImageKey(3),
            color: color(0xff00ff00),
        });
        s.push(Primitive::Image {
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            key: ImageKey(3),
            corner_radius: 4.0,
            rounded: false,
            border_width: 1.0,
            border_color: color(0xffff0000),
        });
        let mut list = DrawList::new();
        scene_to_drawlist(&s, &mut list, &mut fake);
        assert_eq!(list.items.len(), 3);
        assert_eq!(
            list.items[0],
            DrawCmd::Text {
                origin: GPoint::new(4.0, 20.0),
                run: TextRunId(70),
                color: [1.0; 4],
                clip: Some(GRect::new(0.0, -9999.0, 30.0, 19998.0)),
            }
        );
        assert_eq!(
            list.items[1],
            DrawCmd::Image {
                rect: GRect::new(0.0, 0.0, 20.0, 20.0),
                image: ImageId(3),
                corner_radius: 4.0,
                border_width: 1.0,
                border_color: [1.0, 0.0, 0.0, 1.0],
                tint: [0.0, 1.0, 0.0, 1.0],
                nearest: true,
            }
        );
        match &list.items[2] {
            DrawCmd::Image {
                corner_radius,
                border_width,
                tint,
                ..
            } => {
                assert_eq!((*corner_radius, *border_width), (0.0, 0.0));
                assert_eq!(*tint, TRANSPARENT);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn standalone_mask_and_graph_and_hole() {
        let mut fake = Fake::default();
        fake.images.push(5);
        let mut s = Scene::default();
        s.push(Primitive::ImageMask {
            rect: Rect::new(1.0, 1.0, 8.0, 8.0),
            key: ImageKey(5),
            color: color(0xffffffff),
        });
        let line = vec![
            Point::new(0.0, 10.0),
            Point::new(0.0, 5.0),
            Point::new(1.0, 7.0),
        ];
        let mut fill = line.clone();
        fill.push(Point::new(1.0, 20.0));
        fill.push(Point::new(0.0, 20.0));
        s.push(Primitive::Graph {
            line: line.clone(),
            fill: fill.clone(),
            line_color: color(0xffffffff),
            fill_color: color(0x80ffffff),
            line_width: 1.5,
        });
        s.push(Primitive::Graph {
            line: line.clone(),
            fill,
            line_color: color(0xffffffff),
            fill_color: color(0x00000000),
            line_width: 1.0,
        });
        s.push(Primitive::ClipHole {
            rect: Rect::new(10.0, 2.0, 20.0, 16.0),
            corner_radius: 5.0,
            alpha: 0.5,
            stroke_width: 2.0,
            stroke_alpha: 1.0,
        });
        s.push(Primitive::BlurRegion {
            rect: Rect::new(0.0, 0.0, 30.0, 20.0),
            corner_radius: 0.0,
            radius: 20,
        });
        let mut list = DrawList::new();
        scene_to_drawlist(&s, &mut list, &mut fake);
        assert_eq!(list.items.len(), 4);
        assert!(matches!(
            list.items[0],
            DrawCmd::Image { image: ImageId(5), tint, .. } if tint == [1.0; 4]
        ));
        match &list.items[1] {
            DrawCmd::Path {
                points,
                baseline,
                fill,
                line_width,
                ..
            } => {
                assert_eq!(*baseline, 20.0);
                assert!(*fill);
                assert_eq!(*line_width, 1.5);
                assert_eq!(list.path_points(points), &[
                    GPoint::new(0.0, 10.0),
                    GPoint::new(0.0, 5.0),
                    GPoint::new(1.0, 7.0)
                ]);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(list.items[2], DrawCmd::Path { fill: false, .. }));
        assert_eq!(
            list.items[3],
            DrawCmd::Erase {
                rect: GRect::new(10.0, 2.0, 20.0, 16.0),
                corner_radius: 5.0,
                alpha: 0.5,
                stroke_width: 2.0,
                stroke_alpha: 1.0,
            }
        );
        let blurs: Vec<_> = blur_regions(&s).collect();
        assert_eq!(blurs.len(), 1);
        assert_eq!(blurs[0].radius, 20);

        // Reuse keeps capacity (no steady-state allocation).
        let (ci, cp) = (list.items.capacity(), list.points.capacity());
        scene_to_drawlist(&s, &mut list, &mut fake);
        assert_eq!((list.items.capacity(), list.points.capacity()), (ci, cp));
    }

    #[test]
    fn metrics_roundtrip_core_rounding() {
        let l = TextLayout {
            run: TextRunId(1),
            width: 31.0,
            ink_width: 31.0,
            ink_height: 12.0,
            typographic_width: 33.0,
            ink_x: -3.0,
            ink_y: -2.0,
            ascent: 13.2,
            descent: 3.4,
            has_color_glyphs: false,
        };
        let m = text_metrics(TextKey(9), false, &l);
        // The core's rounding (text.rs `prepare_line`) must reproduce the gfx values.
        assert_eq!((m.ink.x as f64 + 0.5) as i32, -3);
        assert_eq!((m.ink.y as f64 + 0.5) as i32, -2);
        assert_eq!((m.ink.width as f64 + 1.5) as u32, 31);
        assert_eq!((m.ink.height as f64 + 1.5) as u32, 12);
        assert_eq!((m.typographic_width as f64 + 0.5) as u32, 33);
        assert_eq!((m.ascent, m.descent, m.key), (13.2, 3.4, TextKey(9)));
        let e = text_metrics(TextKey(1), true, &l);
        assert_eq!(e.ink, Rect::ZERO);
    }

    #[test]
    fn sys_events() {
        let map = |e| sys_event_to_input(e);
        assert_eq!(
            map(SysEvent::FrontAppSwitched {
                name: Some("Safari".into()),
                bundle_id: Some("com.apple.Safari".into()),
                pid: 4
            }),
            Some(Input::Event(OsEvent::FrontAppSwitched {
                name: Some("Safari".into()),
                bundle_id: Some("com.apple.Safari".into())
            }))
        );
        assert_eq!(
            map(SysEvent::SpaceChange {
                info_json: "{}".into()
            }),
            Some(Input::Event(OsEvent::SpaceChanged))
        );
        assert_eq!(
            map(SysEvent::SpaceWindowsChange {
                space: 2,
                info_json: "x".into()
            }),
            Some(Input::Event(OsEvent::SpaceWindowsChanged("x".into())))
        );
        assert_eq!(
            map(SysEvent::DisplayChange { adid: 2 }),
            Some(Input::Event(OsEvent::ActiveDisplayChanged))
        );
        assert_eq!(
            map(SysEvent::DisplaysReconfigured {
                display: 1,
                flags: 0
            }),
            Some(Input::DisplaysChanged)
        );
        assert_eq!(
            map(SysEvent::MenuBarHidingChanged),
            Some(Input::Event(OsEvent::MenuBarHiddenChanged))
        );
        assert_eq!(
            map(SysEvent::VolumeChange(0.5)),
            Some(Input::Event(OsEvent::VolumeChanged(0.5)))
        );
        assert_eq!(
            map(SysEvent::BrightnessChange(0.25)),
            Some(Input::Event(OsEvent::BrightnessChanged(0.25)))
        );
        assert_eq!(
            map(SysEvent::PowerSourceChange("AC".into())),
            Some(Input::Event(OsEvent::PowerSourceChanged(PowerSource::Ac)))
        );
        assert_eq!(
            map(SysEvent::PowerSourceChange("BATTERY".into())),
            Some(Input::Event(OsEvent::PowerSourceChanged(
                PowerSource::Battery
            )))
        );
        assert_eq!(map(SysEvent::PowerSourceChange("UPS".into())), None);
        assert_eq!(
            map(SysEvent::WifiChange("Home".into())),
            Some(Input::Event(OsEvent::WifiChanged("Home".into())))
        );
        assert_eq!(
            map(SysEvent::MediaChange("{}".into())),
            Some(Input::Event(OsEvent::MediaChanged("{}".into())))
        );
        assert_eq!(
            map(SysEvent::SystemWillSleep),
            Some(Input::Event(OsEvent::SystemWillSleep))
        );
        assert_eq!(
            map(SysEvent::SystemWoke {
                screen_unlocked: true
            }),
            Some(Input::Event(OsEvent::SystemWoke))
        );
        assert_eq!(
            map(SysEvent::DistributedNotification {
                name: "com.x".into(),
                user_info_json: Some("{}".into())
            }),
            Some(Input::Event(OsEvent::DistributedNotification {
                name: "com.x".into(),
                info: Some("{}".into())
            }))
        );
        assert_eq!(
            map(SysEvent::MenusChanged {
                app: "Finder".into(),
                pid: 1,
                titles: vec!["Apple".into(), "File".into()]
            }),
            Some(Input::MenuTitles {
                app: "Finder".into(),
                titles: vec!["Apple".into(), "File".into()]
            })
        );
        assert_eq!(
            map(SysEvent::ProviderSample {
                id: 12,
                provider: "cpu".into(),
                values: vec![("percent".into(), "3".into())]
            }),
            Some(Input::ProviderSample {
                item: ItemId(12),
                values: vec![("percent".into(), "3".into())]
            })
        );
        assert_eq!(
            map(SysEvent::CaptureGating { disabled: true }),
            Some(Input::Event(OsEvent::CaptureDisabled(true)))
        );
        assert_eq!(
            map(SysEvent::ConfigChanged),
            Some(Input::Event(OsEvent::ConfigChanged))
        );
        assert_eq!(
            map(SysEvent::ScriptFinished {
                id: 1,
                pid: 99,
                status: Some(0),
                output: Some("o".into()),
                duration: std::time::Duration::from_millis(3),
                timed_out: false
            }),
            Some(Input::ScriptFinished {
                pid: 99,
                item: None,
                output: Some("o".into())
            })
        );
        assert_eq!(map(SysEvent::MediaArtwork(None)), None);
    }

    fn sys_mouse(kind: SysMouseKind, cg_type: u32, button: i64) -> SysMouse {
        SysMouse {
            kind,
            location: CGPoint { x: 100.0, y: 5.0 },
            button,
            cg_type,
            modifiers: 0x100 | 0x20000,
            window_number: 42,
            global: false,
        }
    }

    #[test]
    fn mouse_mapping() {
        let up = monitor_mouse_input(
            &sys_mouse(SysMouseKind::Up, 4, 1),
            Some(WindowKey::Bar(1)),
        )
        .unwrap();
        assert_eq!(
            up,
            MouseInput {
                kind: MouseKind::Up {
                    button: MouseButton::Right,
                    button_code: 1
                },
                point: Point::new(100.0, 5.0),
                window: Some(WindowKey::Bar(1)),
                modifiers: 0x20100,
            }
        );
        assert!(monitor_mouse_input(&sys_mouse(SysMouseKind::Down, 1, 0), None).is_none());
        let sc = monitor_mouse_input(
            &sys_mouse(
                SysMouseKind::Scrolled {
                    delta: -2,
                    point_delta: -14.0,
                },
                22,
                0,
            ),
            None,
        )
        .unwrap();
        assert_eq!(sc.kind, MouseKind::Scrolled { delta: -2 });
        assert_eq!(
            button_of_cg_type(26),
            MouseButton::Other
        );

        let ev = WinMouse {
            kind: MouseEventKind::Moved,
            button: WinButton::None,
            location: GPoint::new(10.0, 3.0),
            modifiers: Modifiers::default(),
            scroll_delta: (0.0, 0.0),
            precise_scroll: false,
            click_count: 0,
        };
        let m = window_mouse_input(WindowKey::Popup(ItemId(3)), Point::new(200.0, 30.0), &ev)
            .unwrap();
        assert_eq!(m.kind, MouseKind::Moved);
        assert_eq!(m.point, Point::new(210.0, 33.0));
        assert_eq!(m.window, Some(WindowKey::Popup(ItemId(3))));
        let up = WinMouse {
            kind: MouseEventKind::Up,
            button: WinButton::Other(2),
            ..ev
        };
        assert_eq!(
            window_mouse_input(WindowKey::Bar(1), Point::new(0.0, 0.0), &up)
                .unwrap()
                .kind,
            MouseKind::Up {
                button: MouseButton::Other,
                button_code: 2
            }
        );
        let down = WinMouse {
            kind: MouseEventKind::Down,
            ..ev
        };
        assert!(window_mouse_input(WindowKey::Bar(1), Point::new(0.0, 0.0), &down).is_none());
        for (k, want) in [
            (MouseEventKind::Entered, MouseKind::Entered),
            (MouseEventKind::Exited, MouseKind::Exited),
            (MouseEventKind::Dragged, MouseKind::Dragged),
        ] {
            let e = WinMouse { kind: k, ..ev };
            assert_eq!(
                window_mouse_input(WindowKey::Bar(1), Point::new(0.0, 0.0), &e)
                    .unwrap()
                    .kind,
                want
            );
        }
        assert_eq!(view_scroll_lines(1.0, false), 1);
        assert_eq!(view_scroll_lines(-2.4, false), -2);
        assert_eq!(view_scroll_lines(35.0, true), 4);
        assert_eq!(view_scroll_lines(3.0, true), 1);
        assert!(monitored_kind(MouseEventKind::Up) && !monitored_kind(MouseEventKind::Moved));
    }

    #[test]
    fn keys() {
        assert_eq!(image_id(image_key(ImageId(7))), Some(ImageId(7)));
        assert_eq!(image_id(ImageKey(0)), None);
        assert_eq!(image_id(ImageKey(u64::MAX)), None);
        assert_eq!(window_level(-20), -20isize);
    }
}
