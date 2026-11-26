//! Title bars for stacking/tabbed containers: drawn into shared-memory buffers
//! and attached to the container's visible window as a river decoration, so
//! they move in the same frame as the window.

use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use chaophraya_tree::Rect;
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use wayland_client::protocol::{wl_buffer, wl_shm, wl_surface};
use wayland_client::QueueHandle;

use crate::config::Color;
use crate::protocol::river_decoration_v1::RiverDecorationV1;
use crate::wm::{State, WinId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabState {
    /// The focused window's tab.
    Focused,
    /// Visible tab of a container that doesn't have focus.
    Active,
    Inactive,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Window the tab stands for (clicking the tab focuses it).
    pub window: WinId,
    /// Relative to the bar's top-left corner.
    pub rect: Rect,
    pub text: String,
    pub state: TabState,
}

/// One title bar to show this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct BarSpec {
    /// Window the bar is attached to (the container's visible window).
    pub owner: WinId,
    /// Global position and size of the whole bar.
    pub rect: Rect,
    pub rows: Vec<Row>,
}

pub struct Palette {
    pub focused: (Color, Color),
    pub active: (Color, Color),
    pub inactive: (Color, Color),
    pub separator: Color,
}

pub struct Text {
    fonts: FontSystem,
    cache: SwashCache,
    family: String,
    px: f32,
}

impl Text {
    /// `font` is "<family> <size in pt>", e.g. "Noto Sans 10".
    pub fn new(font: &str) -> Text {
        let (family, size) = match font.rsplit_once(' ') {
            Some((f, s)) if s.parse::<f32>().is_ok() => (f.to_owned(), s.parse::<f32>().unwrap()),
            _ => (font.to_owned(), 10.0),
        };
        Text { fonts: FontSystem::new(), cache: SwashCache::new(), family, px: size * 96.0 / 72.0 }
    }

    /// Draw `text` left-aligned and vertically centred in `area`, clipped to it.
    fn draw(&mut self, px: &mut [u32], stride: usize, area: Rect, text: &str, fg: Color) {
        let metrics = Metrics::new(self.px, (self.px * 1.25).ceil());
        let mut buf = Buffer::new(&mut self.fonts, metrics);
        buf.set_wrap(&mut self.fonts, Wrap::None);
        buf.set_size(&mut self.fonts, None, Some(metrics.line_height));
        buf.set_text(&mut self.fonts, text, Attrs::new().family(Family::Name(&self.family)), Shaping::Advanced);
        buf.shape_until_scroll(&mut self.fonts, false);
        let [r, g, b, _] = fg.0;
        let pad = 6;
        let ox = area.x + pad;
        let oy = area.y + ((area.h as f32 - metrics.line_height) / 2.0).round() as i32;
        let (x0, x1) = (area.x + pad, area.x + area.w - pad);
        let (y0, y1) = (area.y, area.y + area.h);
        let rows = px.len() / stride;
        buf.draw(&mut self.fonts, &mut self.cache, cosmic_text::Color::rgb(r, g, b), |gx, gy, w, h, c| {
            let a = c.a() as u32;
            if a == 0 {
                return;
            }
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    let (x, y) = (ox + gx + dx, oy + gy + dy);
                    if x < x0 || x >= x1 || y < y0 || y >= y1 || y as usize >= rows || x as usize >= stride {
                        continue;
                    }
                    let i = y as usize * stride + x as usize;
                    px[i] = blend(px[i], c.r(), c.g(), c.b(), a);
                }
            }
        });
    }
}

fn argb(c: Color) -> u32 {
    let [r, g, b, a] = c.0.map(u32::from);
    // Pre-multiplied ARGB8888.
    (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255)
}

fn blend(dst: u32, r: u8, g: u8, b: u8, a: u32) -> u32 {
    let mix = |d: u32, s: u8| (s as u32 * a + d * (255 - a)) / 255;
    let (dr, dg, db) = ((dst >> 16) & 0xff, (dst >> 8) & 0xff, dst & 0xff);
    0xff00_0000 | (mix(dr, r) << 16) | (mix(dg, g) << 8) | mix(db, b)
}

/// Rasterise a bar into ARGB8888 pixels.
pub fn paint(text: &mut Text, spec: &BarSpec, pal: &Palette) -> Vec<u32> {
    let (w, h) = (spec.rect.w.max(1) as usize, spec.rect.h.max(1) as usize);
    let mut px = vec![argb(pal.inactive.0); w * h];
    let sep = argb(pal.separator);
    for row in &spec.rows {
        let (bg, fg) = match row.state {
            TabState::Focused => pal.focused,
            TabState::Active => pal.active,
            TabState::Inactive => pal.inactive,
        };
        let bgc = argb(bg);
        let r = row.rect;
        for y in r.y.max(0)..(r.y + r.h).min(h as i32) {
            for x in r.x.max(0)..(r.x + r.w).min(w as i32) {
                let last_row = y == r.y + r.h - 1;
                let last_col = x == r.x + r.w - 1 && r.x + r.w < w as i32;
                px[y as usize * w + x as usize] = if last_row || last_col { sep } else { bgc };
            }
        }
        text.draw(&mut px, w, Rect::new(r.x, r.y, r.w, r.h - 1), &row.text, fg);
    }
    px
}

/// A decoration surface attached to one window.
pub struct Deco {
    pub surface: wl_surface::WlSurface,
    pub deco: RiverDecorationV1,
    pub last: Option<BarSpec>,
    pub offset: Option<(i32, i32)>,
}

impl Deco {
    pub fn destroy(self) {
        self.deco.destroy();
        self.surface.destroy();
    }
}

/// Upload pixels into a fresh shm buffer (destroyed again when released).
pub fn upload(shm: &wl_shm::WlShm, qh: &QueueHandle<State>, w: i32, h: i32, px: &[u32]) -> Option<wl_buffer::WlBuffer> {
    let size = px.len() * 4;
    let fd = unsafe { libc::memfd_create(c"chaophraya-bar".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let bytes: Vec<u8> = px.iter().flat_map(|p| p.to_le_bytes()).collect();
    let mut file = std::fs::File::from(fd);
    std::io::Write::write_all(&mut file, &bytes).ok()?;
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_and_argb() {
        assert_eq!(argb(Color([0xff, 0x6b, 0, 0xff])), 0xffff6b00);
        assert_eq!(blend(0xff000000, 255, 255, 255, 255), 0xffffffff);
        assert_eq!(blend(0xff000000, 255, 255, 255, 0), 0xff000000);
    }
}
