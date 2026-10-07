//! Line graph of pushed samples (`graph.c`, `docs/spec/components.md` §8).

use super::{color_anim_set, color_set_prop, hex};
use crate::color::Color;
use crate::geometry::Rect;
use crate::props::{AnimValue, PropCx, PropEffects, PropError, PropResult};
use crate::value::{self, split_key, KeySplit};
use std::fmt::Write;

/// `struct graph`.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    /// Number of samples (= width in points), fixed by `--add graph <name> <pos> <width>`.
    pub width: u32,
    /// Ring buffer `y[0..width)`, zero-filled. Deep-copied on clone (D6).
    pub samples: Vec<f32>,
    /// Next write position.
    pub cursor: usize,
    /// `graph.color`, default `0xffcccccc`.
    pub line_color: Color,
    /// `graph.fill_color`, default `0xffcccccc`.
    pub fill_color: Color,
    /// Set only by the plain `graph.fill_color=` setter.
    pub overrides_fill_color: bool,
    /// Default 0.5.
    pub line_width: f32,
    /// Always true (no property).
    pub fill: bool,
    /// Always true (no property).
    pub enabled: bool,
    /// Layout: true for positions `r` and `q` (§8.4); popup items keep the last value.
    pub rtl: bool,
    /// Layout output: `(x, y - H/2 + line_width, width, H)` (§8.4), item-local CG coords.
    pub bounds: Rect,
}

impl Default for Graph {
    fn default() -> Self {
        Graph {
            width: 0,
            samples: Vec::new(),
            cursor: 0,
            line_color: Color::from_hex(0xffcccccc),
            fill_color: Color::from_hex(0xffcccccc),
            overrides_fill_color: false,
            line_width: 0.5,
            fill: true,
            enabled: true,
            rtl: false,
            bounds: Rect::ZERO,
        }
    }
}

/// Largest graph width (samples = points) `--add graph` accepts; larger widths are clamped
/// to it (D22). Wider than any display, small enough that the buffer and its `--query`
/// output stay cheap.
pub const MAX_GRAPH_WIDTH: u32 = 4096;

impl Graph {
    /// `graph_setup(width)`. D22: `width` is clamped to [`MAX_GRAPH_WIDTH`], so a client
    /// command such as `--add graph g left 4294967295` cannot abort the daemon on a 17 GB
    /// allocation.
    pub fn setup(&mut self, width: u32) {
        let width = width.min(MAX_GRAPH_WIDTH);
        self.width = width;
        self.samples = vec![0.0; width as usize];
        self.cursor = 0;
    }

    /// `graph_push_back`: `y[cursor] = v; cursor = (cursor+1) % width`. D5: ignored for a
    /// zero-width graph. Values are not clamped.
    pub fn push(&mut self, v: f32) {
        if !self.enabled || self.samples.is_empty() {
            return;
        }
        self.samples[self.cursor] = v;
        self.cursor = (self.cursor + 1) % self.samples.len();
    }

    /// `graph_get_y(i)`: logical sample `i` (0 = oldest).
    pub fn sample(&self, i: usize) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.samples[(self.cursor + i) % self.samples.len()]
    }

    /// The effective fill colour: `fill_color` if overridden, else the line colour with
    /// alpha × 0.2 (`graph_draw`).
    pub fn effective_fill(&self) -> Color {
        if self.overrides_fill_color {
            self.fill_color
        } else {
            let mut c = self.line_color;
            c.set_alpha(c.a * 0.2);
            c
        }
    }

    /// `graph_parse_sub_domain` (§8.3). Plain colours and `line_width` are not animated;
    /// the colour sub-domains are.
    pub fn set_prop(&mut self, key: &str, v: &str, cx: &mut PropCx) -> PropResult {
        Ok(match key {
            "color" => self.line_color.set_hex(value::parse_u32(v)),
            "fill_color" => {
                self.overrides_fill_color = true;
                self.fill_color.set_hex(value::parse_u32(v))
            }
            "line_width" => {
                self.line_width = value::parse_float(v);
                true
            }
            _ => match split_key(key) {
                KeySplit::Sub("color", rest) => {
                    return cx.scoped("color", |cx| {
                        color_set_prop(&mut self.line_color, rest, v, cx)
                    })
                }
                KeySplit::Sub("fill_color", rest) => {
                    return cx.scoped("fill_color", |cx| {
                        color_set_prop(&mut self.fill_color, rest, v, cx)
                    })
                }
                KeySplit::Sub(sub, _) => {
                    return Err(PropError::GraphInvalidSubdomain(sub.to_string()))
                }
                _ => {
                    return Err(PropError::GraphInvalidProperty(
                        value::display_key(key).to_string(),
                    ))
                }
            },
        })
    }

    pub fn anim_set(&mut self, path: &str, v: AnimValue, fx: &mut PropEffects) -> bool {
        match split_key(path) {
            KeySplit::Sub("color", rest) => color_anim_set(&mut self.line_color, rest, v, fx),
            KeySplit::Sub("fill_color", rest) => color_anim_set(&mut self.fill_color, rest, v, fx),
            _ => false,
        }
    }

    /// `graph_serialize` at indent `g` (§8.6): raw ring-buffer order, quoted `%f`.
    pub fn write_json(&self, out: &mut String, g: &str) {
        let _ = write!(
            out,
            "{g}\"color\": \"{}\",\n{g}\"fill_color\": \"{}\",\n{g}\"line_width\": \"{}\",\n{g}\"data\": [\n",
            hex(self.line_color.hex),
            hex(self.fill_color.hex),
            value::fmt_f(self.line_width as f64)
        );
        for (n, y) in self.samples.iter().enumerate() {
            if n > 0 {
                out.push_str(",\n");
            }
            let _ = write!(out, "{g}\t\"{}\"", value::fmt_f(*y as f64));
        }
        let _ = write!(out, "\n{g}]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_and_json() {
        let mut g = Graph::default();
        g.push(1.0); // D5: ignored
        g.setup(3);
        g.push(0.25);
        g.push(0.5);
        g.push(0.75);
        g.push(1.0);
        assert_eq!(g.samples, vec![1.0, 0.5, 0.75]);
        assert_eq!(g.sample(0), 0.5);
        assert_eq!(g.sample(2), 1.0);
        let mut out = String::new();
        g.write_json(&mut out, "\t\t");
        assert_eq!(
            out,
            "\t\t\"color\": \"0xffcccccc\",\n\t\t\"fill_color\": \"0xffcccccc\",\n\t\t\"line_width\": \"0.500000\",\n\
\t\t\"data\": [\n\t\t\t\"1.000000\",\n\t\t\t\"0.500000\",\n\t\t\t\"0.750000\"\n\t\t]"
        );
        let mut out = String::new();
        Graph::default().write_json(&mut out, "\t\t");
        assert!(out.ends_with("\"data\": [\n\n\t\t]"));
        assert_eq!(g.effective_fill().hex >> 24, (255.0f32 * 0.2) as u32);
    }

    #[test]
    fn setup_clamps_width() {
        let mut g = Graph::default();
        g.setup(u32::MAX);
        assert_eq!(g.width, MAX_GRAPH_WIDTH);
        assert_eq!(g.samples.len(), MAX_GRAPH_WIDTH as usize);
        g.setup(MAX_GRAPH_WIDTH);
        assert_eq!(g.width, MAX_GRAPH_WIDTH);
        g.setup(MAX_GRAPH_WIDTH - 1);
        assert_eq!(g.samples.len(), MAX_GRAPH_WIDTH as usize - 1);
    }
}
