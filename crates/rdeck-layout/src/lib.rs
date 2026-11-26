//! Compositor-independent layout math.
//!
//! Nothing in this crate knows about Wayland or river. It turns
//! "n windows in a w×h area with these parameters" into rectangles, and parses
//! the text commands that change the parameters. The river-classic layout
//! generator (`rdeck`) is one front end; a future river 0.5 window manager can
//! reuse this crate unchanged.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Where the main area sits, or a layout that has no main/stack split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    Left,
    Right,
    Top,
    Bottom,
    /// Main area on the left; stack windows piled on top of each other on the
    /// right, each shifted down by `deck_offset` so its top strip shows.
    Deck,
    /// Every window fills the whole area.
    Monocle,
}

impl Location {
    /// Short name reported to the compositor (shown by e.g. waybar's
    /// `river/layout` module).
    pub fn label(self) -> &'static str {
        match self {
            Location::Left => "[M-S]",
            Location::Right => "[S-M]",
            Location::Top => "[M/S]",
            Location::Bottom => "[S/M]",
            Location::Deck => "[ D ]",
            Location::Monocle => "[ M ]",
        }
    }
}

impl FromStr for Location {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "left" => Location::Left,
            "right" => Location::Right,
            "top" => Location::Top,
            "bottom" => Location::Bottom,
            "deck" => Location::Deck,
            "monocle" => Location::Monocle,
            _ => {
                return Err(format!(
                    "unknown location '{s}' (expected left, right, top, bottom, deck or monocle)"
                ))
            }
        })
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Location::Left => "left",
            Location::Right => "right",
            Location::Top => "top",
            Location::Bottom => "bottom",
            Location::Deck => "deck",
            Location::Monocle => "monocle",
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub main_location: Location,
    pub main_count: u32,
    /// Fraction of the area given to the main windows, 0.1..=0.9.
    pub main_ratio: f64,
    /// Gap between neighbouring windows.
    pub inner_gaps: u32,
    /// Gap between windows and the edge of the usable area.
    pub outer_gaps: u32,
    /// Drop all gaps when only one window is visible (single window or monocle).
    pub smart_gaps: bool,
    /// Vertical shift between stacked cards in the deck layout. 0 = plain
    /// dwm-style deck where cards overlap exactly.
    pub deck_offset: u32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            main_location: Location::Left,
            main_count: 1,
            main_ratio: 0.6,
            inner_gaps: 6,
            outer_gaps: 6,
            smart_gaps: true,
            deck_offset: 0,
        }
    }
}

const MIN_RATIO: f64 = 0.1;
const MAX_RATIO: f64 = 0.9;

/// Value of a command argument: `+N`/`-N` are relative, anything else absolute.
enum Arg<T> {
    Abs(T),
    Rel(T),
}

fn parse_arg<T: FromStr>(arg: &str) -> Result<Arg<T>, String> {
    let bad = || format!("invalid number '{arg}'");
    if arg.starts_with('+') || arg.starts_with('-') {
        // Keep the sign: "-0.1" parses as a negative relative change.
        arg.trim_start_matches('+').parse().map(Arg::Rel).map_err(|_| bad())
    } else {
        arg.parse().map(Arg::Abs).map_err(|_| bad())
    }
}

fn adjust_u32(cur: u32, arg: &str, min: u32) -> Result<u32, String> {
    let v: i64 = match parse_arg::<i64>(arg)? {
        Arg::Abs(v) => v,
        Arg::Rel(d) => cur as i64 + d,
    };
    Ok(v.clamp(min as i64, u32::MAX as i64) as u32)
}

impl Params {
    pub fn label(&self) -> &'static str {
        self.main_location.label()
    }

    /// Apply one text command, e.g. `main-ratio +0.05` or
    /// `main-location-cycle left,deck,monocle`.
    pub fn apply_command(&mut self, command: &str) -> Result<(), String> {
        let mut words = command.split_whitespace();
        let name = words.next().ok_or("empty command")?;
        let arg = words.next();
        if words.next().is_some() {
            return Err(format!("too many arguments in '{command}'"));
        }
        let need = || arg.ok_or_else(|| format!("'{name}' needs an argument"));

        match name {
            "main-ratio" => {
                let v = match parse_arg::<f64>(need()?)? {
                    Arg::Abs(v) => v,
                    Arg::Rel(d) => self.main_ratio + d,
                };
                if !v.is_finite() {
                    return Err("main-ratio must be a number".into());
                }
                self.main_ratio = v.clamp(MIN_RATIO, MAX_RATIO);
            }
            "main-count" => self.main_count = adjust_u32(self.main_count, need()?, 1)?,
            "inner-gaps" => self.inner_gaps = adjust_u32(self.inner_gaps, need()?, 0)?,
            "outer-gaps" => self.outer_gaps = adjust_u32(self.outer_gaps, need()?, 0)?,
            "deck-offset" => self.deck_offset = adjust_u32(self.deck_offset, need()?, 0)?,
            "smart-gaps" => {
                self.smart_gaps = match need()? {
                    "on" => true,
                    "off" => false,
                    "toggle" => !self.smart_gaps,
                    other => return Err(format!("smart-gaps: expected on, off or toggle, got '{other}'")),
                }
            }
            "main-location" => self.main_location = need()?.parse()?,
            "main-location-cycle" => {
                let list = need()?
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<Vec<Location>, _>>()?;
                // Next entry after the current one; the first if current isn't listed.
                let next = match list.iter().position(|&l| l == self.main_location) {
                    Some(i) => list[(i + 1) % list.len()],
                    None => list[0],
                };
                self.main_location = next;
            }
            _ => return Err(format!("unknown command '{name}'")),
        }
        Ok(())
    }
}

/// Split `len` into `n` pieces separated by `gap`. The first piece takes any
/// rounding remainder. Returns (offset, length) pairs.
fn split(len: u32, n: u32, gap: u32) -> impl Iterator<Item = (u32, u32)> {
    let avail = len.saturating_sub(gap.saturating_mul(n.saturating_sub(1)));
    let base = avail / n.max(1);
    let rem = avail % n.max(1);
    let mut off = 0u32;
    (0..n).map(move |i| {
        let size = if i == 0 { base + rem } else { base };
        let item = (off, size);
        off += size + gap;
        item
    })
}

/// Tile `n` windows along one axis of `r`.
fn tile(r: Rect, n: u32, gap: u32, vertical: bool, out: &mut Vec<Rect>) {
    if vertical {
        out.extend(split(r.h, n, gap).map(|(o, h)| Rect { y: r.y + o as i32, h, ..r }));
    } else {
        out.extend(split(r.w, n, gap).map(|(o, w)| Rect { x: r.x + o as i32, w, ..r }));
    }
}

/// Pile `n` cards in `r`, each shifted down by `offset` (shrunk so that cards
/// stay at least half the area tall).
fn deck(r: Rect, n: u32, offset: u32, out: &mut Vec<Rect>) {
    if n <= 1 {
        out.extend((0..n).map(|_| r));
        return;
    }
    let off = offset.min((r.h / 2) / (n - 1));
    let h = r.h - off * (n - 1);
    out.extend((0..n).map(|i| Rect { y: r.y + (i * off) as i32, h, ..r }));
}

/// Compute rectangles for `n` windows in a `width`×`height` area whose origin
/// is (0, 0). The result is in window order: main windows first, then stack.
pub fn layout(p: &Params, n: u32, width: u32, height: u32) -> Vec<Rect> {
    let mut out = Vec::with_capacity(n as usize);
    if n == 0 {
        return out;
    }

    let solo = n == 1 || p.main_location == Location::Monocle;
    let (outer, inner) = if p.smart_gaps && solo { (0, 0) } else { (p.outer_gaps, p.inner_gaps) };
    let area = Rect {
        x: outer as i32,
        y: outer as i32,
        w: width.saturating_sub(2 * outer),
        h: height.saturating_sub(2 * outer),
    };

    if p.main_location == Location::Monocle {
        out.extend((0..n).map(|_| area));
        return out;
    }

    let main_n = p.main_count.min(n);
    let stack_n = n - main_n;
    // Left/Right/Deck put main and stack side by side; Top/Bottom stack them.
    let side_by_side = matches!(p.main_location, Location::Left | Location::Right | Location::Deck);

    let (main_area, stack_area) = if stack_n == 0 {
        (Some(area), None)
    } else if main_n == 0 {
        (None, Some(area))
    } else {
        let total = if side_by_side { area.w } else { area.h };
        let avail = total.saturating_sub(inner);
        let want = (avail as f64 * p.main_ratio).round() as u32;
        // Leave at least 1px for each side when there is room for it.
        let main_len = if avail < 2 { avail } else { want.clamp(1, avail - 1) };
        let stack_len = avail - main_len;
        let first = |len: u32| if side_by_side { Rect { w: len, ..area } } else { Rect { h: len, ..area } };
        let second = |len: u32, skip: u32| {
            if side_by_side {
                Rect { x: area.x + skip as i32, w: len, ..area }
            } else {
                Rect { y: area.y + skip as i32, h: len, ..area }
            }
        };
        match p.main_location {
            Location::Left | Location::Top | Location::Deck => {
                (Some(first(main_len)), Some(second(stack_len, main_len + inner)))
            }
            _ => (Some(second(main_len, stack_len + inner)), Some(first(stack_len))),
        }
    };

    if let Some(r) = main_area {
        tile(r, main_n, inner, side_by_side, &mut out);
    }
    if let Some(r) = stack_area {
        if p.main_location == Location::Deck {
            deck(r, stack_n, p.deck_offset, &mut out);
        } else {
            tile(r, stack_n, inner, side_by_side, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Params {
        Params { inner_gaps: 0, outer_gaps: 0, main_ratio: 0.5, ..Params::default() }
    }

    fn r(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn single_window_fills_area_with_smart_gaps() {
        let p = Params::default();
        assert_eq!(layout(&p, 1, 1000, 800), vec![r(0, 0, 1000, 800)]);
    }

    #[test]
    fn left_master_stack() {
        assert_eq!(
            layout(&p(), 3, 1000, 800),
            vec![r(0, 0, 500, 800), r(500, 0, 500, 400), r(500, 400, 500, 400)]
        );
    }

    #[test]
    fn right_and_top() {
        let mut q = p();
        q.main_location = Location::Right;
        assert_eq!(layout(&q, 2, 1000, 800), vec![r(500, 0, 500, 800), r(0, 0, 500, 800)]);
        q.main_location = Location::Top;
        assert_eq!(
            layout(&q, 3, 1000, 800),
            vec![r(0, 0, 1000, 400), r(0, 400, 500, 400), r(500, 400, 500, 400)]
        );
    }

    #[test]
    fn gaps_are_exact() {
        let q = Params { inner_gaps: 10, outer_gaps: 5, main_ratio: 0.5, smart_gaps: false, ..Params::default() };
        // area 990x790 at (5,5); main/stack split 990-10=980 -> 490/490
        assert_eq!(
            layout(&q, 3, 1000, 800),
            vec![r(5, 5, 490, 790), r(505, 5, 490, 390), r(505, 405, 490, 390)]
        );
    }

    #[test]
    fn deck_offsets_cards() {
        let q = Params { main_location: Location::Deck, deck_offset: 30, ..p() };
        assert_eq!(
            layout(&q, 4, 1000, 800),
            vec![r(0, 0, 500, 800), r(500, 0, 500, 740), r(500, 30, 500, 740), r(500, 60, 500, 740)]
        );
    }

    #[test]
    fn deck_offset_shrinks_to_keep_cards_tall() {
        let q = Params { main_location: Location::Deck, deck_offset: 100, ..p() };
        let rects = layout(&q, 11, 1000, 800); // 10 cards: offset capped at 400/9 = 44
        assert_eq!(rects[1], r(500, 0, 500, 800 - 44 * 9));
        assert_eq!(rects[10].y, 44 * 9);
    }

    #[test]
    fn monocle_and_counts() {
        let q = Params { main_location: Location::Monocle, ..Params::default() };
        assert_eq!(layout(&q, 3, 100, 50), vec![r(0, 0, 100, 50); 3]);
        assert!(layout(&q, 0, 100, 50).is_empty());
        let q = Params { main_count: 2, ..p() };
        assert_eq!(layout(&q, 2, 1000, 800), vec![r(0, 0, 1000, 400), r(0, 400, 1000, 400)]);
    }

    #[test]
    fn tiny_area_does_not_panic() {
        for loc in ["left", "right", "top", "bottom", "deck", "monocle"] {
            let q = Params { main_location: loc.parse().unwrap(), deck_offset: 50, ..Params::default() };
            for n in 0..6 {
                for (w, h) in [(0, 0), (1, 1), (5, 3), (13, 2)] {
                    assert_eq!(layout(&q, n, w, h).len(), n as usize);
                }
            }
        }
    }

    #[test]
    fn commands() {
        let mut q = Params::default();
        q.apply_command("main-ratio 0.5").unwrap();
        q.apply_command("main-ratio -0.1").unwrap();
        assert!((q.main_ratio - 0.4).abs() < 1e-9);
        q.apply_command("main-ratio +5").unwrap();
        assert_eq!(q.main_ratio, MAX_RATIO);
        q.apply_command("main-count -5").unwrap();
        assert_eq!(q.main_count, 1);
        q.apply_command("main-count +2").unwrap();
        assert_eq!(q.main_count, 3);
        q.apply_command("smart-gaps toggle").unwrap();
        assert!(!q.smart_gaps);
        q.apply_command("deck-offset 28").unwrap();
        assert_eq!(q.deck_offset, 28);
        for want in [Location::Deck, Location::Monocle, Location::Left, Location::Deck] {
            q.apply_command("main-location-cycle left,deck,monocle").unwrap();
            assert_eq!(q.main_location, want);
        }
        q.apply_command("main-location right").unwrap();
        q.apply_command("main-location-cycle left,deck,monocle").unwrap();
        assert_eq!(q.main_location, Location::Left);
        assert!(q.apply_command("main-location sideways").is_err());
        assert!(q.apply_command("bogus 1").is_err());
        assert!(q.apply_command("main-ratio").is_err());
        assert!(q.apply_command("main-ratio nan").is_err());
    }
}
