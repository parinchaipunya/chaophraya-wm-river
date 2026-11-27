//! Window manager state and the river manage/render cycle.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::process::Child;

use chaophraya_tree::{Axis, Dir, Rect, Style, TitleBar, Tree};
use wayland_backend::client::ObjectId;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::WEnum;
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{self, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use crate::command::{Command, MAX_WORKSPACE};
use crate::config::{Config, IdleMode, Size};
use crate::deco::{self, BarSpec, Deco, Palette, Row, TabState, Text};
use crate::protocol::river_decoration_v1::RiverDecorationV1;
use crate::protocol::river_layer_shell_output_v1::{self, RiverLayerShellOutputV1};
use crate::protocol::river_layer_shell_seat_v1::{self, RiverLayerShellSeatV1};
use crate::protocol::river_layer_shell_v1::RiverLayerShellV1;
use crate::protocol::river_node_v1::RiverNodeV1;
use crate::protocol::river_output_v1::{self, RiverOutputV1};
use crate::protocol::river_pointer_binding_v1::{self, RiverPointerBindingV1};
use crate::protocol::river_seat_v1::{self, RiverSeatV1};
use crate::protocol::river_window_manager_v1::{self, RiverWindowManagerV1};
use crate::protocol::river_window_v1::{self, Edges, RiverWindowV1};
use crate::protocol::river_xkb_binding_v1::{self, RiverXkbBindingV1};
use crate::protocol::river_xkb_bindings_v1::RiverXkbBindingsV1;

pub type WinId = u32;

/// Workspace 0 holds windows parked in the scratchpad.
const SCRATCH: u8 = 0;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

fn all_edges() -> Edges {
    Edges::Top | Edges::Bottom | Edges::Left | Edges::Right
}

pub struct Window {
    proxy: RiverWindowV1,
    node: RiverNodeV1,
    pub app_id: String,
    pub title: String,
    has_parent: bool,
    fixed_size: bool,
    pub ws: u8,
    pub floating: bool,
    pub sticky: bool,
    /// Lives in the scratchpad (shown or parked).
    scratch: bool,
    pub fullscreen: bool,
    /// Wants attention (set via `urgent app_id`, cleared on focus).
    pub urgent: bool,
    /// Floating content rectangle (global coordinates).
    float: Rect,
    float_placed: bool,
    float_size: Option<Size>,
    inhibit_idle: Option<IdleMode>,
    /// From a rule's border_width; None uses general.border_width.
    border_width: Option<i32>,
    pub dims: (i32, i32),
    new: bool,
    closed: bool,
    proposed: Option<(i32, i32)>,
    pub shown: Option<bool>,
    tiled_edges_sent: Option<bool>,
    /// Content position to apply in the next render sequence.
    pub target: Option<(i32, i32)>,
    last_cell: Rect,
    fullscreen_req: Option<Option<RiverOutputV1>>,
    exit_fullscreen_req: bool,
    move_req: Option<RiverSeatV1>,
    resize_req: Option<(RiverSeatV1, Edges)>,
}

pub struct Output {
    proxy: RiverOutputV1,
    layer: Option<RiverLayerShellOutputV1>,
    wl_name: Option<u32>,
    rect: Rect,
    area: Option<Rect>,
    pub ws: u8,
    /// Other workspaces shown beside `ws` (view toggle); `ws` is the active
    /// one of them, the one with the focus.
    pub view: Vec<u8>,
    removed: bool,
    /// Same position and size as another output: it shows that output's
    /// content (river renders the same area), so it gets no workspace.
    pub mirror: bool,
    /// Parked (see park_outputs_except): on, but no workspace.
    pub parked: bool,
    /// The parking rule has been applied (needs the output's name).
    park_decided: bool,
    title_bars: Vec<TitleBar<WinId>>,
}

#[derive(Clone)]
enum Op {
    None,
    Move { win: WinId, start: (i32, i32) },
    Resize { win: WinId, start: Rect, edges: Edges },
    /// Super+right-drag on a tiled window: move its edges nearest the cursor.
    /// `applied` is the part of the drag already turned into resizes.
    ResizeTiled { win: WinId, h: Option<Dir>, v: Option<Dir>, applied: (i32, i32) },
}

struct Seat {
    proxy: RiverSeatV1,
    layer: Option<RiverLayerShellSeatV1>,
    new_bindings: bool,
    removed: bool,
    xkb: Vec<(RiverXkbBindingV1, Command)>,
    /// Parallel to `xkb`: stays active on the lock screen, and its mode.
    xkb_meta: Vec<(bool, Option<String>)>,
    /// (locked, mode) the enabled/disabled bindings currently reflect.
    bindings_state: Option<(bool, String)>,
    pointer: Vec<RiverPointerBindingV1>,
    hovered: Option<WinId>,
    hover_changed: bool,
    /// Last pointer position and whether it moved since the last manage
    /// sequence; focus only follows the mouse when the mouse actually moves.
    pointer_pos: Option<(i32, i32)>,
    pointer_moved: bool,
    interacted: Option<WinId>,
    layer_exclusive: bool,
    op: Op,
    op_delta: (i32, i32),
    op_release: bool,
    ptr_action: Option<u32>,
    focus_sent: Option<Option<WinId>>,
}

/// A command waiting for the next manage sequence; `client` gets the reply.
pub struct Pending {
    pub command: Command,
    pub client: Option<u64>,
}

pub struct State {
    pub config: Config,
    pub wm: Option<RiverWindowManagerV1>,
    xkb: Option<RiverXkbBindingsV1>,
    layer_shell: Option<RiverLayerShellV1>,
    registry: Option<wl_registry::WlRegistry>,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    /// Font machinery, loaded on the first title bar.
    text: Option<Text>,
    decos: HashMap<WinId, Deco>,
    bars: Vec<BarSpec>,
    pub windows: BTreeMap<WinId, Window>,
    by_proxy: HashMap<ObjectId, WinId>,
    next_id: WinId,
    pub outputs: Vec<Output>,
    /// wl_output global name -> connector name (eDP-1, ...).
    output_names: HashMap<u32, String>,
    seats: Vec<Seat>,
    pub trees: Vec<Tree<WinId>>,
    /// Last focused window per workspace (also covers floating windows).
    ws_focus: Vec<Option<WinId>>,
    pub focused: Option<WinId>,
    pub focused_output: Option<ObjectId>,
    pub queue: Vec<Pending>,
    /// Key bindings pressed per seat, run in the next manage sequence.
    binding_actions: HashMap<ObjectId, Vec<Command>>,
    /// Replies for IPC clients produced during the last manage sequence.
    pub replies: Vec<(u64, String)>,
    pub state_changed: bool,
    children: Vec<Child>,
    pub exit: Option<String>,
    pub input: crate::input::Input,
    reload_inputs: bool,
    session_locked: bool,
    /// Current binding mode ("default" or a [modes.NAME] table).
    pub mode: String,
    cursor_mgr: Option<WpCursorShapeManagerV1>,
    /// The WM's own pointers (for clicks on title bars).
    pointers: Vec<(wl_pointer::WlPointer, Option<WpCursorShapeDeviceV1>)>,
    /// Title-bar surface under the pointer and the pointer position on it.
    pointer_on: Option<(ObjectId, f64, f64)>,
    /// Tab (window) under the pointer when the left button went down.
    tab_pressed: Option<WinId>,
    /// A command moved focus to another output: warp the cursor there.
    warp_pending: bool,
    /// Hidden scratchpad windows, next to show first (sway-style cycling).
    scratch_order: VecDeque<WinId>,
    /// Workspace -> output name pins set at runtime (`workspace N output NAME`),
    /// taking precedence over [workspace_outputs] in the config.
    ws_pins: HashMap<u8, String>,
    /// Some window's inhibit_idle rule currently applies.
    idle_inhibited: bool,
    /// Windows un-hidden in this manage sequence.
    newly_shown: Vec<WinId>,
    /// Focus went to a window that was still hidden; send it again next time.
    refocus: bool,
}

impl State {
    pub fn new(config: Config) -> State {
        State {
            config,
            wm: None,
            xkb: None,
            layer_shell: None,
            registry: None,
            compositor: None,
            shm: None,
            text: None,
            decos: HashMap::new(),
            bars: Vec::new(),
            windows: BTreeMap::new(),
            by_proxy: HashMap::new(),
            next_id: 1,
            outputs: Vec::new(),
            output_names: HashMap::new(),
            seats: Vec::new(),
            trees: (0..=MAX_WORKSPACE).map(|_| Tree::new()).collect(),
            ws_focus: vec![None; MAX_WORKSPACE as usize + 1],
            focused: None,
            focused_output: None,
            queue: Vec::new(),
            binding_actions: HashMap::new(),
            replies: Vec::new(),
            state_changed: true,
            children: Vec::new(),
            exit: None,
            input: Default::default(),
            reload_inputs: false,
            session_locked: false,
            mode: "default".into(),
            idle_inhibited: false,
            ws_pins: HashMap::new(),
            scratch_order: VecDeque::new(),
            warp_pending: false,
            cursor_mgr: None,
            pointers: Vec::new(),
            pointer_on: None,
            tab_pressed: None,
            newly_shown: Vec::new(),
            refocus: false,
        }
    }

    // ---- helpers ----------------------------------------------------------

    pub fn spawn(&mut self, cmd: &str) {
        match std::process::Command::new("sh").arg("-c").arg(cmd).env_remove("WAYLAND_DEBUG").spawn() {
            Ok(child) => self.children.push(child),
            Err(e) => eprintln!("chaophraya: spawn '{cmd}': {e}"),
        }
    }

    /// Show an error as a desktop notification (e.g. a failed reload).
    pub fn notify_error(&mut self, msg: &str) {
        match std::process::Command::new("notify-send")
            .args(["-u", "critical", "-a", "chaophraya", "chaophraya", msg])
            .spawn()
        {
            Ok(child) => self.children.push(child),
            Err(e) => eprintln!("chaophraya: notify-send: {e}"),
        }
    }

    /// Collect exited children so they don't linger as zombies.
    pub fn reap(&mut self) {
        self.children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    }

    pub fn output_name(&self, o: &Output) -> String {
        o.wl_name.and_then(|n| self.output_names.get(&n).cloned()).unwrap_or_default()
    }

    fn output_idx(&self, id: &ObjectId) -> Option<usize> {
        self.outputs.iter().position(|o| &o.proxy.id() == id)
    }

    fn ws_output_idx(&self, ws: u8) -> Option<usize> {
        if ws == SCRATCH {
            return None;
        }
        self.outputs.iter().position(|o| (o.ws == ws || o.view.contains(&ws)) && !o.removed)
    }

    /// Make `ws`, shown in output `i`'s view, the active workspace there.
    fn activate(&mut self, i: usize, ws: u8) {
        let o = &mut self.outputs[i];
        if o.ws != ws && o.view.contains(&ws) {
            o.view.retain(|v| *v != ws);
            let old = o.ws;
            o.view.push(old);
            o.ws = ws;
            self.state_changed = true;
        }
    }

    /// Stop showing other workspaces beside `ws` on output `i`; their sticky
    /// windows come along to `to`.
    fn clear_view(&mut self, i: usize, to: u8) {
        let members = std::mem::take(&mut self.outputs[i].view);
        let active = self.outputs[i].ws;
        for v in members.into_iter().chain(std::iter::once(active)) {
            if v != to && v != 0 {
                self.migrate_sticky(v, to);
            }
        }
    }

    /// Workspaces shown on output `i` and the part of it each gets: the whole
    /// output for one, side by side (or a grid, for more than three) for a
    /// combined view.
    fn regions(&self, i: usize) -> Vec<(u8, Rect)> {
        let o = &self.outputs[i];
        if o.ws == 0 {
            return Vec::new();
        }
        let g = &self.config.general;
        let area = self.usable_area(i).inset(g.gaps_outer);
        let mut shown: Vec<u8> = o.view.iter().copied().chain(std::iter::once(o.ws)).collect();
        shown.sort_unstable();
        shown.dedup();
        let n = shown.len() as i32;
        if n == 1 {
            return vec![(o.ws, area)];
        }
        let cols = if n <= 3 { n } else { (n as f64).sqrt().ceil() as i32 };
        let rows = (n + cols - 1) / cols;
        let gap = g.gaps_inner.max(0);
        shown
            .into_iter()
            .enumerate()
            .map(|(k, ws)| {
                let (r, c) = (k as i32 / cols, k as i32 % cols);
                // The last row spreads its regions over the full width.
                let in_row = if r == rows - 1 { n - r * cols } else { cols };
                let x0 = area.x + area.w * c / in_row;
                let x1 = area.x + area.w * (c + 1) / in_row;
                let y0 = area.y + area.h * r / rows;
                let y1 = area.y + area.h * (r + 1) / rows;
                let (gl, gt) = (if c > 0 { gap / 2 } else { 0 }, if r > 0 { gap / 2 } else { 0 });
                let (gr, gb) = (if c < in_row - 1 { gap - gap / 2 } else { 0 }, if r < rows - 1 { gap - gap / 2 } else { 0 });
                (ws, Rect::new(x0 + gl, y0 + gt, x1 - x0 - gl - gr, y1 - y0 - gt - gb))
            })
            .collect()
    }

    /// Area floating windows of each shown workspace are placed in.
    fn float_areas(&self) -> HashMap<u8, Rect> {
        let mut m = HashMap::new();
        for i in 0..self.outputs.len() {
            if self.outputs[i].removed {
                continue;
            }
            let regions = self.regions(i);
            if regions.len() == 1 {
                m.insert(regions[0].0, self.usable_area(i));
            } else {
                m.extend(regions);
            }
        }
        m
    }

    /// Workspace shown next to `ws` in direction `dir` on the same output.
    fn region_in_dir(&self, ws: u8, dir: Dir) -> Option<u8> {
        let i = self.ws_output_idx(ws)?;
        let regions = self.regions(i);
        let from = regions.iter().find(|(w, _)| *w == ws)?.1;
        let c = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
        let (cx, cy) = c(from);
        regions
            .iter()
            .filter(|(w, _)| *w != ws)
            .filter_map(|(w, r)| {
                let (x, y) = c(*r);
                let (dx, dy) = (x - cx, y - cy);
                let ok = match dir {
                    Dir::Left => dx < 0 && dx.abs() >= dy.abs(),
                    Dir::Right => dx > 0 && dx.abs() >= dy.abs(),
                    Dir::Up => dy < 0 && dy.abs() > dx.abs(),
                    Dir::Down => dy > 0 && dy.abs() > dx.abs(),
                };
                ok.then_some((*w, dx.abs() + dy.abs()))
            })
            .min_by_key(|(_, d)| *d)
            .map(|(w, _)| w)
    }

    /// Focus workspace `ws` (shown on output `i`): its best window, or the
    /// empty region itself.
    fn focus_region(&mut self, i: usize, ws: u8) {
        self.activate(i, ws);
        self.focused_output = Some(self.outputs[i].proxy.id());
        let f = self.ws_focus_candidate(ws);
        self.focused = None;
        self.set_focus(f);
        self.state_changed = true;
    }

    /// `view toggle N`: show or hide workspace N beside the current one.
    fn view_toggle(&mut self, n: u8) -> Result<(), String> {
        let i = self.focused_output_idx().ok_or("no output")?;
        if self.outputs[i].ws == n {
            // Hiding the active workspace: the next shown one takes over.
            if self.outputs[i].view.is_empty() {
                return Ok(());
            }
            self.outputs[i].view.sort_unstable();
            let next = self.outputs[i].view.remove(0);
            self.outputs[i].ws = next;
            self.migrate_sticky(n, next);
            self.focus_region(i, next);
        } else if self.outputs[i].view.contains(&n) {
            self.outputs[i].view.retain(|v| *v != n);
            let ws = self.outputs[i].ws;
            self.migrate_sticky(n, ws);
        } else if let Some(j) = self.ws_output_idx(n) {
            return Err(format!("workspace {n} is shown on {}", self.output_name(&self.outputs[j])));
        } else {
            self.outputs[i].view.push(n);
        }
        self.state_changed = true;
        Ok(())
    }

    /// `view toggle all`: show every workspace that has windows (and isn't on
    /// another output); if they are all shown already, hide all but the
    /// active one. The scratchpad is not included.
    fn view_toggle_all(&mut self) -> Result<(), String> {
        let i = self.focused_output_idx().ok_or("no output")?;
        let cand: Vec<u8> = (1..=MAX_WORKSPACE)
            .filter(|n| self.windows.values().any(|w| w.ws == *n))
            .filter(|n| self.ws_output_idx(*n).is_none_or(|j| j == i))
            .collect();
        let o = &self.outputs[i];
        if cand.iter().all(|n| o.ws == *n || o.view.contains(n)) {
            let ws = self.outputs[i].ws;
            self.clear_view(i, ws);
        } else {
            for n in cand {
                let o = &mut self.outputs[i];
                if o.ws != n && !o.view.contains(&n) {
                    o.view.push(n);
                }
            }
        }
        self.state_changed = true;
        Ok(())
    }

    /// `scratchpad show all`: show every hidden scratchpad window on the
    /// current workspace (cascaded); if none is hidden, hide them all.
    fn scratchpad_show_all(&mut self) {
        let cur = self.current_ws();
        let mut hidden: Vec<WinId> = self
            .scratch_order
            .iter()
            .copied()
            .filter(|id| self.windows.get(id).is_some_and(|w| w.scratch && w.ws == SCRATCH))
            .collect();
        for (id, w) in &self.windows {
            if w.scratch && w.ws == SCRATCH && !hidden.contains(id) {
                hidden.push(*id);
            }
        }
        if hidden.is_empty() {
            let shown: Vec<WinId> =
                self.windows.iter().filter(|(_, w)| w.scratch && w.ws != SCRATCH).map(|(id, _)| *id).collect();
            for id in shown {
                self.make_floating_for_scratch(id);
                self.windows.get_mut(&id).unwrap().ws = SCRATCH;
                self.scratch_order.retain(|x| *x != id);
                self.scratch_order.push_back(id);
            }
            let f = self.ws_focus_candidate(cur);
            self.focused = None;
            self.set_focus(f);
        } else {
            let area = self.float_areas().get(&cur).copied();
            for (k, id) in hidden.iter().enumerate() {
                let w = self.windows.get_mut(id).unwrap();
                w.ws = cur;
                match area {
                    Some(a) if w.float.w > 0 && w.float.h > 0 => {
                        let step = 30 * k as i32;
                        let r = center(a, w.float.w, w.float.h);
                        w.float = Rect::new(r.x + step, r.y + step, r.w, r.h);
                        w.float_placed = true;
                    }
                    _ => w.float_placed = false,
                }
            }
            self.set_focus(hidden.last().copied());
        }
        self.state_changed = true;
    }

    pub fn ws_visible(&self, ws: u8) -> bool {
        self.ws_output_idx(ws).is_some()
    }

    fn focused_output_idx(&self) -> Option<usize> {
        if let Some(w) = self.focused.and_then(|f| self.windows.get(&f)) {
            if let Some(i) = self.ws_output_idx(w.ws) {
                return Some(i);
            }
        }
        self.focused_output
            .as_ref()
            .and_then(|id| self.output_idx(id))
            .or_else(|| self.outputs.iter().position(usable))
    }

    /// The workspace the user is looking at.
    pub fn current_ws(&self) -> u8 {
        self.focused_output_idx().map(|i| self.outputs[i].ws).unwrap_or(1)
    }

    fn usable_area(&self, i: usize) -> Rect {
        let o = &self.outputs[i];
        o.area.unwrap_or(o.rect)
    }

    fn set_focus(&mut self, w: Option<WinId>) {
        if let Some(id) = w {
            if let Some(win) = self.windows.get_mut(&id) {
                if std::mem::take(&mut win.urgent) {
                    self.state_changed = true;
                }
                let win = &self.windows[&id];
                let ws = win.ws;
                if !win.floating {
                    self.trees[ws as usize].set_focus(id);
                }
                self.ws_focus[ws as usize] = Some(id);
                if let Some(i) = self.ws_output_idx(ws) {
                    self.activate(i, ws);
                    self.focused_output = Some(self.outputs[i].proxy.id());
                }
            }
        }
        if self.focused != w {
            self.focused = w;
            self.state_changed = true;
        }
    }

    /// Best window to focus on workspace `ws`.
    fn ws_focus_candidate(&self, ws: u8) -> Option<WinId> {
        let remembered = self.ws_focus[ws as usize].filter(|id| self.windows.get(id).is_some_and(|w| w.ws == ws));
        remembered
            .or_else(|| self.trees[ws as usize].focused())
            .or_else(|| self.windows.iter().rev().find(|(_, w)| w.ws == ws).map(|(id, _)| *id))
    }

    fn insert_tiled(&mut self, id: WinId) {
        let w = &self.windows[&id];
        let tree = &mut self.trees[w.ws as usize];
        if w.sticky {
            tree.insert_sticky(id);
        } else {
            tree.insert(id);
        }
    }

    fn remove_tiled(&mut self, id: WinId) {
        if let Some(w) = self.windows.get(&id) {
            if w.ws != SCRATCH {
                self.trees[w.ws as usize].remove(id);
            }
        }
    }

    /// Show workspace `n`: focus it where it is visible, otherwise put it on
    /// the focused output (or the output configured for it).
    fn show_workspace(&mut self, n: u8) {
        if let Some(i) = self.ws_output_idx(n) {
            // Switching workspace ends a combined view.
            if !self.outputs[i].view.is_empty() {
                self.activate(i, n);
                self.clear_view(i, n);
            }
            self.focused_output = Some(self.outputs[i].proxy.id());
            let f = self.ws_focus_candidate(n);
            self.set_focus(f);
            self.focused = f;
            self.state_changed = true;
            return;
        }
        let pin = self.ws_pins.get(&n).or_else(|| self.config.workspace_outputs.get(&n)).cloned();
        let preferred = pin.and_then(|name| self.output_by_name(&name));
        let Some(i) = preferred.or_else(|| self.focused_output_idx()) else { return };
        let old = self.outputs[i].ws;
        self.clear_view(i, old);
        self.outputs[i].ws = n;
        self.migrate_sticky(old, n);
        self.focused_output = Some(self.outputs[i].proxy.id());
        let f = self.ws_focus_candidate(n);
        self.focused = None;
        self.set_focus(f);
        self.state_changed = true;
    }

    /// Sticky windows follow their output from workspace `from` to `to`.
    fn migrate_sticky(&mut self, from: u8, to: u8) {
        if from == to || from == SCRATCH {
            return;
        }
        let sticky: Vec<WinId> =
            self.windows.iter().filter(|(_, w)| w.sticky && w.ws == from).map(|(id, _)| *id).collect();
        for id in sticky {
            let floating = self.windows[&id].floating;
            if !floating {
                self.trees[from as usize].remove(id);
            }
            self.windows.get_mut(&id).unwrap().ws = to;
            if !floating {
                self.trees[to as usize].insert_sticky(id);
            }
        }
    }

    /// Index of the usable (not removed, mirrored or parked) output `name`.
    fn output_by_name(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|o| usable(o) && self.output_name(o) == name)
    }

    /// Park or unpark an output: a parked output keeps running (so monitors
    /// that reconnect when their signal stops don't bounce) but shows no
    /// workspace and is skipped by focus, moves and cursor warps.
    fn set_parked(&mut self, i: usize, parked: bool) {
        if self.outputs[i].parked == parked {
            return;
        }
        self.outputs[i].parked = parked;
        if parked {
            let ws = self.outputs[i].ws;
            self.clear_view(i, ws);
            self.outputs[i].ws = 0;
            if self.focused_output == Some(self.outputs[i].proxy.id()) {
                self.focused_output = None;
                let f = self.ws_focus_candidate(self.current_ws());
                self.focused = None;
                self.set_focus(f);
            }
        }
        // Unparked outputs get a workspace in init_outputs.
        self.state_changed = true;
    }

    /// Apply park_outputs_except to outputs whose name just became known.
    fn update_parking(&mut self) {
        for i in 0..self.outputs.len() {
            if self.outputs[i].park_decided || self.outputs[i].removed {
                continue;
            }
            let name = self.output_name(&self.outputs[i]);
            if name.is_empty() {
                continue; // wl_output name not received yet
            }
            self.outputs[i].park_decided = true;
            let keep = &self.config.general.park_outputs_except;
            if !keep.is_empty() && !keep.contains(&name) {
                self.set_parked(i, true);
            }
        }
        // Never leave no usable output (e.g. only the external is connected):
        // once every output's name is known, unpark one if needed.
        let all_decided = self.outputs.iter().all(|o| o.removed || o.park_decided);
        if all_decided && !self.outputs.iter().any(usable) {
            if let Some(i) = self.outputs.iter().position(|o| !o.removed && o.parked) {
                self.set_parked(i, false);
            }
        }
    }

    /// Move the focused workspace to output `name`; the output it leaves
    /// shows the lowest workspace that isn't visible anywhere.
    fn move_workspace_to_output(&mut self, name: &str) -> Result<(), String> {
        let n = self.current_ws();
        let target = self.output_by_name(name).ok_or_else(|| format!("no output named {name}"))?;
        if self.outputs[target].ws == n {
            return Ok(());
        }
        let source = self.ws_output_idx(n);
        let old = self.outputs[target].ws;
        self.outputs[target].ws = n;
        if let Some(s) = source {
            if self.outputs[s].view.is_empty() {
                let free = (1..=MAX_WORKSPACE).find(|w| *w != n && !self.ws_visible(*w)).unwrap_or(old);
                self.outputs[s].ws = free;
            } else {
                self.outputs[s].view.sort_unstable();
                self.outputs[s].ws = self.outputs[s].view.remove(0);
            }
        }
        self.focused_output = Some(self.outputs[target].proxy.id());
        let f = self.ws_focus_candidate(n);
        self.focused = None;
        self.set_focus(f);
        self.state_changed = true;
        Ok(())
    }

    /// Mark outputs that exactly overlap another one as mirrors (and give
    /// back a workspace to outputs that stopped mirroring).
    fn update_mirrors(&mut self) {
        // Among outputs at the same position and size, one shows the
        // workspace: an output listed in park_outputs_except (the laptop) if
        // there is one, otherwise the first; the others are its mirrors.
        let keep = self.config.general.park_outputs_except.clone();
        let rank = |s: &Self, i: usize| -> (bool, std::cmp::Reverse<usize>) {
            (keep.contains(&s.output_name(&s.outputs[i])), std::cmp::Reverse(i))
        };
        for i in 0..self.outputs.len() {
            let r = self.outputs[i].rect;
            let mirror = !self.outputs[i].removed
                && r.w > 0
                && !self.outputs[i].parked
                && (0..self.outputs.len()).any(|j| {
                    let o = &self.outputs[j];
                    j != i && !o.removed && !o.parked && o.rect == r && rank(self, j) > rank(self, i)
                });
            if mirror != self.outputs[i].mirror {
                self.outputs[i].mirror = mirror;
                if mirror {
                    // Its workspace stays, just not visible; focus moves on.
                    let ws = self.outputs[i].ws;
                    self.clear_view(i, ws);
                    self.outputs[i].ws = 0;
                    if self.focused_output == Some(self.outputs[i].proxy.id()) {
                        self.focused_output = None;
                    }
                }
                self.state_changed = true;
            }
        }
    }

    /// Output next to output `i` in direction `dir`, by geometry.
    fn output_in_dir(&self, i: usize, dir: Dir) -> Option<usize> {
        let c = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
        let (cx, cy) = c(self.outputs[i].rect);
        self.outputs
            .iter()
            .enumerate()
            .filter(|(j, o)| *j != i && usable(o))
            .filter_map(|(j, o)| {
                let (x, y) = c(o.rect);
                let (dx, dy) = (x - cx, y - cy);
                let ok = match dir {
                    Dir::Left => dx < 0 && dx.abs() >= dy.abs(),
                    Dir::Right => dx > 0 && dx.abs() >= dy.abs(),
                    Dir::Up => dy < 0 && dy.abs() > dx.abs(),
                    Dir::Down => dy > 0 && dy.abs() > dx.abs(),
                };
                ok.then_some((j, dx.abs() + dy.abs()))
            })
            .min_by_key(|(_, d)| *d)
            .map(|(j, _)| j)
    }

    // ---- commands ---------------------------------------------------------

    /// Run a command; if it moved focus to another output, warp the cursor
    /// there at the end of this manage sequence (sway's mouse_warping).
    pub fn run_command(&mut self, cmd: &Command) -> Result<(), String> {
        let before = self.focused_output_idx().map(|i| self.outputs[i].proxy.id());
        let result = self.run_command_inner(cmd);
        let after = self.focused_output_idx().map(|i| self.outputs[i].proxy.id());
        if after.is_some() && after != before {
            self.warp_pending = true;
        }
        result
    }

    fn run_command_inner(&mut self, cmd: &Command) -> Result<(), String> {
        let focused = self.focused.filter(|id| self.windows.contains_key(id));
        let fwin = focused.map(|id| (id, self.windows[&id].floating, self.windows[&id].ws));
        match cmd {
            Command::Nop => {}
            Command::Spawn(c) => self.spawn(c),
            Command::Close => {
                if let Some(id) = focused {
                    self.windows[&id].proxy.close();
                }
            }
            Command::Exit => {
                if let Some(wm) = &self.wm {
                    wm.exit_session();
                }
            }
            Command::Reload => {
                let config = Config::load()?;
                self.config = config;
                if !self.config.modes.contains(&self.mode) {
                    self.mode = "default".into();
                }
                for seat in &mut self.seats {
                    seat.new_bindings = true;
                }
                for w in self.windows.values_mut() {
                    w.proposed = None;
                }
                for c in self.config.autostart.exec_always.clone() {
                    self.spawn(&c);
                }
                self.reload_inputs = true;
            }
            Command::Focus(dir) => match fwin {
                Some((_, false, ws)) => {
                    if let Some(w) = self.trees[ws as usize].focus_dir(*dir) {
                        self.set_focus(Some(w));
                    } else {
                        self.focus_region_or_output(ws, *dir);
                    }
                }
                Some((_, true, _)) => {}
                None => self.focus_region_or_output(self.current_ws(), *dir),
            },
            Command::FocusModeToggle => {
                if let Some((_, floating, ws)) = fwin {
                    let target = if floating {
                        self.trees[ws as usize].focused()
                    } else {
                        self.windows.iter().rev().find(|(_, w)| w.ws == ws && w.floating).map(|(id, _)| *id)
                    };
                    if target.is_some() {
                        self.set_focus(target);
                    }
                }
            }
            Command::Move(dir) => match fwin {
                Some((id, false, ws)) => {
                    if !self.trees[ws as usize].move_dir(id, *dir) {
                        // At the edge: send it to the neighbouring region of
                        // a combined view, or the neighbouring output.
                        if let Some(t) = self.region_in_dir(ws, *dir) {
                            self.move_to_workspace(id, t);
                            self.set_focus(Some(id));
                        } else if let Some(o) = self.ws_output_idx(ws).and_then(|i| self.output_in_dir(i, *dir)) {
                            let target = self.outputs[o].ws;
                            self.move_to_workspace(id, target);
                            self.focused_output = Some(self.outputs[o].proxy.id());
                            self.set_focus(Some(id));
                        }
                    }
                }
                Some((id, true, _)) => {
                    let step = 20;
                    let w = self.windows.get_mut(&id).unwrap();
                    match dir {
                        Dir::Left => w.float.x -= step,
                        Dir::Right => w.float.x += step,
                        Dir::Up => w.float.y -= step,
                        Dir::Down => w.float.y += step,
                    }
                }
                None => {}
            },
            Command::Layout(l) => {
                if let Some((id, false, ws)) = fwin {
                    self.trees[ws as usize].set_layout(id, *l);
                }
            }
            Command::LayoutToggleSplit => {
                if let Some((id, false, ws)) = fwin {
                    self.trees[ws as usize].toggle_split(id);
                }
            }
            Command::Split(axis) => {
                if let Some((id, false, ws)) = fwin {
                    self.trees[ws as usize].split(id, *axis);
                }
            }
            Command::FullscreenToggle => {
                if let Some(id) = focused {
                    let on = !self.windows[&id].fullscreen;
                    self.set_fullscreen(id, on, None);
                }
            }
            Command::FloatingToggle => {
                if let Some((id, floating, _)) = fwin {
                    if floating {
                        // Tiling a scratchpad window takes it out of the
                        // scratchpad for good (like sway); `move scratchpad`
                        // puts it back.
                        let w = self.windows.get_mut(&id).unwrap();
                        w.floating = false;
                        w.scratch = false;
                        self.scratch_order.retain(|x| *x != id);
                        self.insert_tiled(id);
                    } else {
                        self.remove_tiled(id);
                        let w = self.windows.get_mut(&id).unwrap();
                        w.floating = true;
                        let bw = w.border_width.unwrap_or(self.config.general.border_width);
                        w.float = w.last_cell.inset(bw);
                        w.float_placed = w.float.w > 0;
                        w.proposed = None;
                    }
                    self.set_focus(Some(id));
                }
            }
            Command::StickyToggle => {
                if let Some((id, floating, _)) = fwin {
                    let sticky = !self.windows[&id].sticky;
                    if !floating {
                        self.remove_tiled(id);
                    }
                    self.windows.get_mut(&id).unwrap().sticky = sticky;
                    if !floating {
                        self.insert_tiled(id);
                        self.set_focus(Some(id));
                    }
                }
            }
            Command::Workspace(n) => self.show_workspace(*n),
            Command::FocusId(n) => {
                let Some(w) = self.windows.get(n) else { return Err(format!("no window with id {n}")) };
                let ws = w.ws;
                if ws == SCRATCH {
                    return Err(format!("window {n} is in the scratchpad"));
                }
                if !self.ws_visible(ws) {
                    self.show_workspace(ws);
                }
                self.set_focus(Some(*n));
            }
            Command::KeyboardLayout(i) => self.set_keyboard_layout(*i),
            Command::PinWorkspace(n, name) => {
                self.ws_pins.insert(*n, name.clone());
            }
            Command::MoveWorkspaceToOutput(name) => self.move_workspace_to_output(name)?,
            Command::ParkOutput(name, park) => {
                let i = self
                    .outputs
                    .iter()
                    .position(|o| !o.removed && self.output_name(o) == *name)
                    .ok_or_else(|| format!("no output named {name}"))?;
                self.outputs[i].park_decided = true;
                if *park && self.outputs.iter().filter(|o| usable(o)).count() <= 1 && usable(&self.outputs[i]) {
                    return Err(format!("{name} is the only usable output"));
                }
                self.set_parked(i, *park);
            }
            Command::UrgentAppId(name) => {
                let name = name.to_lowercase();
                let matches = |app_id: &str| {
                    let a = app_id.to_lowercase();
                    !a.is_empty() && (a == name || a.rsplit('.').next() == Some(name.as_str()))
                };
                for w in self.windows.values_mut() {
                    // Only windows you can't see: visible ones need no flag.
                    if matches(&w.app_id) && w.shown != Some(true) {
                        w.urgent = true;
                    }
                }
            }
            Command::MoveToWorkspace(n) => {
                if let Some((id, _, ws)) = fwin {
                    if ws != *n && !self.windows[&id].sticky {
                        self.move_to_workspace(id, *n);
                        let f = self.ws_focus_candidate(ws);
                        self.focused = None;
                        self.set_focus(f);
                    }
                }
            }
            Command::Resize { axis, px } => match fwin {
                Some((id, false, ws)) => {
                    self.trees[ws as usize].resize(id, *axis, *px);
                }
                Some((id, true, _)) => {
                    let w = self.windows.get_mut(&id).unwrap();
                    match axis {
                        Axis::H => w.float.w = (w.float.w + px).max(50),
                        Axis::V => w.float.h = (w.float.h + px).max(50),
                    }
                }
                None => {}
            },
            Command::ScratchpadMove => {
                if let Some((id, floating, ws)) = fwin {
                    if !floating {
                        self.remove_tiled(id);
                    }
                    let w = self.windows.get_mut(&id).unwrap();
                    w.floating = true;
                    w.scratch = true;
                    w.sticky = false;
                    w.ws = SCRATCH;
                    w.float_placed = false;
                    w.proposed = None;
                    self.scratch_order.retain(|x| *x != id);
                    self.scratch_order.push_back(id);
                    let f = self.ws_focus_candidate(ws);
                    self.focused = None;
                    self.set_focus(f);
                }
            }
            Command::ScratchpadShow => self.scratchpad_show(focused),
            Command::ScratchpadShowAll => self.scratchpad_show_all(),
            Command::ViewToggle(Some(n)) => self.view_toggle(*n)?,
            Command::ViewToggle(None) => self.view_toggle_all()?,
            Command::Mode(name) => {
                if name != "default" && !self.config.modes.contains(name) {
                    return Err(format!("no mode named {name}"));
                }
                self.mode = name.clone();
            }
        }
        self.state_changed = true;
        Ok(())
    }

    /// Sway's `scratchpad show`:
    /// - the focused window is a visible scratch window: hide it again (it
    ///   goes to the back of the queue, so repeated presses cycle);
    /// - a scratch window is visible somewhere: bring it to the current
    ///   workspace (if it isn't there already) and focus it;
    /// - otherwise show the next hidden scratch window here.
    fn scratchpad_show(&mut self, focused: Option<WinId>) {
        let cur = self.current_ws();
        if let Some(id) = focused.filter(|id| self.windows[id].scratch && self.windows[id].ws != SCRATCH) {
            self.make_floating_for_scratch(id);
            self.windows.get_mut(&id).unwrap().ws = SCRATCH;
            self.scratch_order.retain(|x| *x != id);
            self.scratch_order.push_back(id);
            let f = self.ws_focus_candidate(cur);
            self.focused = None;
            self.set_focus(f);
            return;
        }
        let shown = self
            .windows
            .iter()
            .filter(|(_, w)| w.scratch && w.ws != SCRATCH)
            .min_by_key(|(_, w)| w.ws != cur)          // prefer one already here
            .map(|(id, _)| *id);
        if let Some(id) = shown {
            if self.windows[&id].ws != cur {
                self.make_floating_for_scratch(id);
                let w = self.windows.get_mut(&id).unwrap();
                w.ws = cur;
                w.float_placed = false;
            }
            self.set_focus(Some(id));
            return;
        }
        let windows = &self.windows;
        self.scratch_order.retain(|id| windows.get(id).is_some_and(|w| w.scratch && w.ws == SCRATCH));
        if let Some(id) = self.scratch_order.pop_front() {
            self.scratch_order.push_back(id);
            let w = self.windows.get_mut(&id).unwrap();
            w.ws = cur;
            w.float_placed = false;
            self.set_focus(Some(id));
        }
    }

    /// A scratch window that was tiled again (`floating toggle`) has to leave
    /// its workspace's tree before it is hidden or moved: the scratchpad only
    /// holds floating windows, like sway's.
    fn make_floating_for_scratch(&mut self, id: WinId) {
        if self.windows.get(&id).is_some_and(|w| !w.floating) {
            self.remove_tiled(id);
            let w = self.windows.get_mut(&id).unwrap();
            w.floating = true;
            w.float_placed = false;
            w.proposed = None;
        }
    }

    /// Move the cursor to the focused output (or window), unless it is
    /// already on that output. The new position is recorded as the cursor's
    /// own, so the warp doesn't count as mouse movement for focus-follows-mouse.
    fn warp_pointer(&mut self) {
        let mode = self.config.general.mouse_warping.as_str();
        if mode == "none" {
            return;
        }
        let Some(i) = self.focused_output_idx() else { return };
        let r = self.outputs[i].rect;
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        let window_centre = (mode == "container")
            .then(|| self.focused.and_then(|id| self.windows.get(&id)))
            .flatten()
            .filter(|w| w.shown == Some(true) && self.ws_output_idx(w.ws) == Some(i))
            .and_then(|w| w.target.map(|(x, y)| (x + w.dims.0 / 2, y + w.dims.1 / 2)));
        let (x, y) = window_centre.unwrap_or((r.x + r.w / 2, r.y + r.h / 2));
        let inside = |p: (i32, i32)| p.0 >= r.x && p.0 < r.x + r.w && p.1 >= r.y && p.1 < r.y + r.h;
        for seat in &mut self.seats {
            if seat.pointer_pos.is_some_and(inside) && mode == "output" {
                continue;
            }
            seat.proxy.pointer_warp(x, y);
            seat.pointer_pos = Some((x, y));
            seat.pointer_moved = false;
        }
    }

    pub fn pointer_position(&self) -> Option<(i32, i32)> {
        self.seats.iter().find_map(|s| s.pointer_pos)
    }

    /// At the edge of workspace `ws`: focus the neighbouring region of a
    /// combined view, or else the neighbouring output.
    fn focus_region_or_output(&mut self, ws: u8, dir: Dir) {
        match self.region_in_dir(ws, dir) {
            Some(t) => {
                if let Some(i) = self.ws_output_idx(t) {
                    self.focus_region(i, t);
                }
            }
            None => self.focus_output_dir(dir),
        }
    }

    fn focus_output_dir(&mut self, dir: Dir) {
        let Some(i) = self.focused_output_idx() else { return };
        if let Some(j) = self.output_in_dir(i, dir) {
            self.focused_output = Some(self.outputs[j].proxy.id());
            let f = self.ws_focus_candidate(self.outputs[j].ws);
            self.focused = None;
            self.set_focus(f);
        }
    }

    fn move_to_workspace(&mut self, id: WinId, n: u8) {
        let floating = self.windows[&id].floating;
        if !floating {
            self.remove_tiled(id);
        }
        let w = self.windows.get_mut(&id).unwrap();
        w.ws = n;
        if floating {
            // Keep the same spot relative to the new output.
            w.float_placed = false;
        } else {
            self.trees[n as usize].insert(id);
        }
        self.ws_focus[n as usize] = Some(id);
    }

    fn set_fullscreen(&mut self, id: WinId, on: bool, output: Option<RiverOutputV1>) {
        let ws = self.windows[&id].ws;
        let out = output.or_else(|| self.ws_output_idx(ws).map(|i| self.outputs[i].proxy.clone()));
        let w = self.windows.get_mut(&id).unwrap();
        if on {
            let Some(out) = out else { return };
            w.proxy.fullscreen(&out);
            w.proxy.inform_fullscreen();
            w.fullscreen = true;
        } else if w.fullscreen {
            w.proxy.exit_fullscreen();
            w.proxy.inform_not_fullscreen();
            w.fullscreen = false;
            w.proposed = None;
        }
    }

    // ---- manage sequence --------------------------------------------------

    fn manage(&mut self, qh: &QueueHandle<State>) {
        self.remove_dead();
        self.update_mirrors();
        self.update_parking();
        self.init_outputs();
        self.init_windows();
        self.init_seats(qh);
        self.handle_window_requests();
        self.handle_seats();
        let queue = std::mem::take(&mut self.queue);
        for p in queue {
            let reply = match self.run_command(&p.command) {
                Ok(()) => "ok".to_owned(),
                Err(e) => format!("error: {e}"),
            };
            if let Some(c) = p.client {
                self.replies.push((c, reply));
            }
        }
        if std::mem::take(&mut self.reload_inputs) {
            self.reapply_inputs(qh);
        }
        if std::mem::take(&mut self.input.apply_pending) {
            self.apply_libinput(qh);
        }
        self.arrange();
        self.apply_focus();
        self.update_idle_inhibit();
        if std::mem::take(&mut self.warp_pending) {
            self.warp_pointer();
        }
        if let Some(wm) = &self.wm {
            wm.manage_finish();
            if std::mem::take(&mut self.refocus) {
                wm.manage_dirty();
            }
        }
    }

    fn remove_dead(&mut self) {
        let dead: Vec<WinId> = self.windows.iter().filter(|(_, w)| w.closed).map(|(id, _)| *id).collect();
        for id in dead {
            // Every tree, not just the one for its workspace: a stale entry
            // would point at a window that no longer exists.
            for tree in &mut self.trees {
                tree.remove(id);
            }
            if let Some(d) = self.decos.remove(&id) {
                d.destroy();
            }
            let w = self.windows.remove(&id).unwrap();
            self.by_proxy.remove(&w.proxy.id());
            w.node.destroy();
            w.proxy.destroy();
            for seat in &mut self.seats {
                if seat.hovered == Some(id) {
                    seat.hovered = None;
                }
                if seat.interacted == Some(id) {
                    seat.interacted = None;
                }
                if matches!(seat.op, Op::Move { win, .. } | Op::Resize { win, .. } | Op::ResizeTiled { win, .. } if win == id) {
                    seat.proxy.op_end();
                    seat.op = Op::None;
                }
            }
            if self.focused == Some(id) {
                let f = self.ws_focus_candidate(w.ws);
                self.focused = None;
                self.set_focus(f);
            }
            self.state_changed = true;
        }
        let mut i = 0;
        while i < self.outputs.len() {
            if self.outputs[i].removed {
                let o = self.outputs.remove(i);
                if let Some(l) = o.layer {
                    l.destroy();
                }
                o.proxy.destroy();
                self.state_changed = true;
            } else {
                i += 1;
            }
        }
        self.seats.retain(|s| {
            if s.removed {
                for (b, _) in &s.xkb {
                    b.destroy();
                }
                for b in &s.pointer {
                    b.destroy();
                }
                if let Some(l) = &s.layer {
                    l.destroy();
                }
                s.proxy.destroy();
            }
            !s.removed
        });
    }

    fn init_outputs(&mut self) {
        for i in 0..self.outputs.len() {
            if self.outputs[i].ws != 0 || self.outputs[i].mirror || self.outputs[i].parked {
                continue;
            }
            // Lowest workspace not shown anywhere.
            let ws = (1..=MAX_WORKSPACE).find(|n| !self.ws_visible(*n)).unwrap_or(MAX_WORKSPACE);
            self.outputs[i].ws = ws;
            if self.focused_output.is_none() {
                self.focused_output = Some(self.outputs[i].proxy.id());
            }
            self.state_changed = true;
        }
        if let (Some(i), Some(_)) = (self.focused_output_idx(), &self.layer_shell) {
            if let Some(l) = &self.outputs[i].layer {
                l.set_default();
            }
        }
    }

    fn init_windows(&mut self) {
        let new: Vec<WinId> = self.windows.iter().filter(|(_, w)| w.new).map(|(id, _)| *id).collect();
        for id in new {
            let cur = self.current_ws();
            let rules: Vec<_> = {
                let w = &self.windows[&id];
                self.config.rules.iter().filter(|r| r.matches(&w.app_id, &w.title)).cloned().collect()
            };
            let w = self.windows.get_mut(&id).unwrap();
            w.new = false;
            w.ws = cur;
            w.floating = w.has_parent || w.fixed_size;
            w.proxy.use_ssd();
            w.proxy.set_capabilities(river_window_v1::Capabilities::Fullscreen);
            let mut kill = false;
            let mut scratch = false;
            for r in rules {
                if let Some(f) = r.floating {
                    w.floating = f;
                }
                if let Some(s) = r.sticky {
                    w.sticky = s;
                }
                if let Some(ws) = r.workspace {
                    w.ws = ws;
                }
                if r.size.is_some() {
                    w.float_size = r.size;
                }
                if r.inhibit_idle.is_some() {
                    w.inhibit_idle = r.inhibit_idle;
                }
                if r.border_width.is_some() {
                    w.border_width = r.border_width;
                }
                kill |= r.kill;
                scratch |= r.scratchpad;
            }
            if kill {
                w.proxy.close();
                w.ws = SCRATCH;
                continue;
            }
            if scratch {
                w.scratch = true;
                w.floating = true;
                w.ws = SCRATCH;
                self.scratch_order.retain(|x| *x != id);
                self.scratch_order.push_back(id);
                continue;
            }
            let (ws, floating) = (w.ws, w.floating);
            if !floating {
                self.insert_tiled(id);
            }
            // Sticky windows always live on the visible workspace.
            if self.windows[&id].sticky && !self.ws_visible(ws) {
                self.migrate_one_sticky(id, cur);
            }
            if self.ws_visible(self.windows[&id].ws) {
                self.set_focus(Some(id));
            } else {
                self.ws_focus[ws as usize] = Some(id);
            }
            self.state_changed = true;
        }
    }

    fn migrate_one_sticky(&mut self, id: WinId, to: u8) {
        let floating = self.windows[&id].floating;
        self.remove_tiled(id);
        self.windows.get_mut(&id).unwrap().ws = to;
        if !floating {
            self.trees[to as usize].insert_sticky(id);
        }
    }

    fn init_seats(&mut self, qh: &QueueHandle<State>) {
        let Some(xkb) = self.xkb.clone() else { return };
        for si in 0..self.seats.len() {
            if !self.seats[si].new_bindings {
                continue;
            }
            let seat = &mut self.seats[si];
            for (b, _) in seat.xkb.drain(..) {
                b.destroy();
            }
            seat.xkb_meta.clear();
            for b in seat.pointer.drain(..) {
                b.destroy();
            }
            for b in &self.config.bindings {
                let proxy = xkb.get_xkb_binding(&seat.proxy, b.keysym, b.mods, qh, seat.proxy.id());
                // Match against the first layout so bindings survive layout switches.
                proxy.set_layout_override(0);
                seat.xkb.push((proxy, b.command.clone()));
                seat.xkb_meta.push((b.locked, b.mode.clone()));
            }
            for button in [BTN_LEFT, BTN_RIGHT] {
                let p = seat.proxy.get_pointer_binding(button, self.config.floating_mod, qh, seat.proxy.id());
                seat.pointer.push(p);
            }
            seat.bindings_state = None;
            seat.new_bindings = false;
        }
        // Only the current mode's bindings are active. On the lock screen
        // only the default mode's `locked:` bindings are (like sway's --locked).
        let locked = self.session_locked;
        let mode = if locked { "default".to_string() } else { self.mode.clone() };
        let want = (locked, mode.clone());
        for seat in self.seats.iter_mut().filter(|s| s.bindings_state.as_ref() != Some(&want)) {
            for ((b, _), (b_locked, b_mode)) in seat.xkb.iter().zip(&seat.xkb_meta) {
                let on = b_mode.as_deref().unwrap_or("default") == mode && (!locked || *b_locked);
                if on { b.enable() } else { b.disable() }
            }
            for p in &seat.pointer {
                if locked { p.disable() } else { p.enable() }
            }
            seat.bindings_state = Some(want.clone());
        }
    }

    fn handle_window_requests(&mut self) {
        let ids: Vec<WinId> = self.windows.keys().copied().collect();
        for id in ids {
            let w = self.windows.get_mut(&id).unwrap();
            if let Some(out) = w.fullscreen_req.take() {
                self.set_fullscreen(id, true, out);
            }
            let w = self.windows.get_mut(&id).unwrap();
            if std::mem::take(&mut w.exit_fullscreen_req) {
                self.set_fullscreen(id, false, None);
            }
            let w = self.windows.get_mut(&id).unwrap();
            let move_req = w.move_req.take();
            let resize_req = w.resize_req.take();
            let floating = w.floating;
            if floating {
                if let Some(seat) = move_req {
                    self.start_op(&seat.id(), id, false, all_edges());
                }
                if let Some((seat, edges)) = resize_req {
                    self.start_op(&seat.id(), id, true, edges);
                }
            }
        }
    }

    fn start_op(&mut self, seat_id: &ObjectId, win: WinId, resize: bool, edges: Edges) {
        let Some(w) = self.windows.get(&win) else { return };
        let (float, dims) = (w.float, w.dims);
        let Some(seat) = self.seats.iter_mut().find(|s| &s.proxy.id() == seat_id) else { return };
        if !matches!(seat.op, Op::None) {
            return;
        }
        seat.proxy.op_start_pointer();
        seat.op_delta = (0, 0);
        seat.op = if resize {
            w.proxy.inform_resize_start();
            Op::Resize { win, start: Rect::new(float.x, float.y, dims.0, dims.1), edges }
        } else {
            Op::Move { win, start: (float.x, float.y) }
        };
    }

    /// Start Super+right-drag resizing of tiled window `win`: pick the
    /// horizontal and vertical edge nearest the pointer (falling back to the
    /// opposite edge when there is no neighbour on that side).
    fn start_tiled_resize(&mut self, si: usize, win: WinId) {
        let Some(w) = self.windows.get(&win) else { return };
        let cell = w.last_cell;
        let ws = w.ws as usize;
        let (px, py) = self.seats[si].pointer_pos.unwrap_or((cell.x + cell.w, cell.y + cell.h));
        let tree = &mut self.trees[ws];
        let mut pick = |near: Dir, far: Dir| {
            // A zero-pixel probe tells whether that edge has a neighbour.
            if tree.resize_edge(win, near, 0) {
                Some(near)
            } else if tree.resize_edge(win, far, 0) {
                Some(far)
            } else {
                None
            }
        };
        let right_half = px >= cell.x + cell.w / 2;
        let lower_half = py >= cell.y + cell.h / 2;
        let h = if right_half { pick(Dir::Right, Dir::Left) } else { pick(Dir::Left, Dir::Right) };
        let v = if lower_half { pick(Dir::Down, Dir::Up) } else { pick(Dir::Up, Dir::Down) };
        if h.is_none() && v.is_none() {
            return; // a lone window: nothing to resize against
        }
        let seat = &mut self.seats[si];
        seat.proxy.op_start_pointer();
        seat.op_delta = (0, 0);
        seat.op = Op::ResizeTiled { win, h, v, applied: (0, 0) };
    }

    fn handle_seats(&mut self) {
        for si in 0..self.seats.len() {
            // Clicks focus (and raise) the window.
            if let Some(id) = self.seats[si].interacted.take() {
                if self.windows.contains_key(&id) {
                    self.set_focus(Some(id));
                }
            }
            self.seats[si].hover_changed = false;
            let moved = std::mem::take(&mut self.seats[si].pointer_moved);
            if moved && self.config.general.focus_follows_mouse {
                if let Some(id) = self.seats[si].hovered {
                    let visible = self.windows.get(&id).is_some_and(|w| w.shown == Some(true));
                    if visible && matches!(self.seats[si].op, Op::None) {
                        self.set_focus(Some(id));
                    }
                }
            }
            if let Some(button) = self.seats[si].ptr_action.take() {
                if let Some(id) = self.seats[si].hovered {
                    if self.windows.get(&id).is_some_and(|w| w.floating) {
                        self.set_focus(Some(id));
                        let seat_id = self.seats[si].proxy.id();
                        self.start_op(&seat_id, id, button == BTN_RIGHT, Edges::Bottom | Edges::Right);
                    } else if button == BTN_RIGHT && matches!(self.seats[si].op, Op::None) {
                        self.set_focus(Some(id));
                        self.start_tiled_resize(si, id);
                    }
                }
            }
            // Super+right-drag on a tiled window: apply the new part of the drag.
            if let Op::ResizeTiled { win, h, v, applied } = self.seats[si].op.clone() {
                let (dx, dy) = self.seats[si].op_delta;
                let ws = self.windows.get(&win).map(|w| w.ws);
                if let Some(ws) = ws.filter(|ws| *ws != SCRATCH) {
                    let tree = &mut self.trees[ws as usize];
                    let mut done = applied;
                    // The edge follows the mouse: moving a right/bottom edge
                    // outward is +delta, a left/top edge outward is -delta.
                    if let Some(e) = h {
                        let step = dx - applied.0;
                        if step != 0 && tree.resize_edge(win, e, if e == Dir::Right { step } else { -step }) {
                            done.0 = dx;
                        }
                    }
                    if let Some(e) = v {
                        let step = dy - applied.1;
                        if step != 0 && tree.resize_edge(win, e, if e == Dir::Down { step } else { -step }) {
                            done.1 = dy;
                        }
                    }
                    self.seats[si].op = Op::ResizeTiled { win, h, v, applied: done };
                }
            }
            // Pointer move/resize of floating windows.
            let seat = &mut self.seats[si];
            match seat.op.clone() {
                Op::None | Op::ResizeTiled { .. } => {}
                Op::Move { win, start } => {
                    if let Some(w) = self.windows.get_mut(&win) {
                        w.float.x = start.0 + seat.op_delta.0;
                        w.float.y = start.1 + seat.op_delta.1;
                    }
                }
                Op::Resize { win, start, edges } => {
                    if let Some(w) = self.windows.get_mut(&win) {
                        let (dx, dy) = seat.op_delta;
                        let mut r = start;
                        if edges.contains(Edges::Left) {
                            r.x += dx;
                            r.w -= dx;
                        }
                        if edges.contains(Edges::Right) {
                            r.w += dx;
                        }
                        if edges.contains(Edges::Top) {
                            r.y += dy;
                            r.h -= dy;
                        }
                        if edges.contains(Edges::Bottom) {
                            r.h += dy;
                        }
                        w.float = Rect::new(r.x, r.y, r.w.max(50), r.h.max(50));
                    }
                }
            }
            if std::mem::take(&mut seat.op_release) {
                if let Op::Resize { win, .. } = seat.op {
                    if let Some(w) = self.windows.get(&win) {
                        w.proxy.inform_resize_end();
                    }
                }
                seat.proxy.op_end();
                seat.op = Op::None;
            }
            // Key bindings.
            let actions: Vec<Command> = std::mem::take(self.pending_actions_of(si));
            for a in actions {
                if let Err(e) = self.run_command(&a) {
                    eprintln!("chaophraya: {e}");
                    // Key presses have no terminal to report to: show it.
                    self.notify_error(&e);
                }
            }
        }
    }

    fn pending_actions_of(&mut self, si: usize) -> &mut Vec<Command> {
        let id = self.seats[si].proxy.id();
        self.binding_actions.entry(id).or_default()
    }

    /// Compute every window's place and visibility for this manage sequence.
    fn arrange(&mut self) {
        let g = self.config.general.clone();
        let mut visible: HashMap<WinId, bool> = self.windows.keys().map(|id| (*id, false)).collect();
        for i in 0..self.outputs.len() {
            let mut title_bars = Vec::new();
            for (ws, area) in self.regions(i) {
                let style = Style { gap: g.gaps_inner, title_height: g.title_height };
                let arr = self.trees[ws as usize].arrange(area, style);
                let stale: Vec<WinId> =
                    arr.windows.iter().map(|(id, _)| *id).filter(|id| !self.windows.contains_key(id)).collect();
                if !stale.is_empty() {
                    // Shouldn't happen; drop the entries instead of crashing the session.
                    eprintln!("chaophraya: workspace {ws}: dropping stale windows {stale:?} from the layout");
                    for id in &stale {
                        self.trees[ws as usize].remove(*id);
                    }
                    self.state_changed = true;
                }
                for (id, cell) in arr.windows {
                    let Some(w) = self.windows.get_mut(&id) else { continue };
                    let content = cell.inset(w.border_width.unwrap_or(g.border_width));
                    w.last_cell = cell;
                    visible.insert(id, true);
                    if !w.fullscreen {
                        let size = (content.w.max(1), content.h.max(1));
                        if w.proposed != Some(size) {
                            w.proxy.propose_dimensions(size.0, size.1);
                            w.proposed = Some(size);
                        }
                        w.target = Some((content.x, content.y));
                    }
                    if w.tiled_edges_sent != Some(true) {
                        w.proxy.set_tiled(all_edges());
                        w.tiled_edges_sent = Some(true);
                    }
                }
                title_bars.extend(arr.title_bars);
            }
            self.outputs[i].title_bars = title_bars;
        }
        self.build_bars();
        // Floating windows on visible workspaces.
        let area_of = self.float_areas();
        for (id, w) in self.windows.iter_mut() {
            if !w.floating || w.ws == SCRATCH {
                continue;
            }
            let Some(area) = area_of.get(&w.ws).copied() else { continue };
            visible.insert(*id, true);
            if w.tiled_edges_sent != Some(false) {
                w.proxy.set_tiled(Edges::None);
                w.tiled_edges_sent = Some(false);
            }
            if w.fullscreen {
                continue;
            }
            let want = if w.float_placed {
                Some((w.float.w, w.float.h))
            } else {
                match w.float_size {
                    Some(Size::Px(fw, fh)) => Some((fw, fh)),
                    Some(Size::Percent(pw, ph)) => Some((area.w * pw / 100, area.h * ph / 100)),
                    None => None,
                }
            };
            let want = want.map(|(a, b)| (a.max(1), b.max(1))).unwrap_or((0, 0));
            if w.proposed != Some(want) {
                w.proxy.propose_dimensions(want.0, want.1);
                w.proposed = Some(want);
            }
            if !w.float_placed && want != (0, 0) {
                w.float = center(area, want.0, want.1);
                w.float_placed = true;
            }
            w.target = w.float_placed.then_some((w.float.x, w.float.y));
        }
        // Fullscreen windows on visible workspaces stay visible too.
        for (id, w) in self.windows.iter() {
            if w.fullscreen && area_of.contains_key(&w.ws) {
                visible.insert(*id, true);
            }
        }
        let colors = self.config.colors.clone();
        for (id, w) in self.windows.iter_mut() {
            let vis = visible[id];
            if w.shown != Some(vis) {
                if vis {
                    w.proxy.show();
                    self.newly_shown.push(*id);
                } else {
                    w.proxy.hide();
                }
                w.shown = Some(vis);
            }
            if vis {
                let c = if self.focused == Some(*id) {
                    colors.focused
                } else if w.urgent {
                    colors.urgent
                } else {
                    colors.unfocused
                };
                let [r, g2, b, a] = c.river();
                w.proxy.set_borders(all_edges(), w.border_width.unwrap_or(g.border_width), r, g2, b, a);
            }
        }
        // Drop the focus if its window is no longer visible.
        if let Some(f) = self.focused {
            if !visible.get(&f).copied().unwrap_or(false) {
                let cur = self.current_ws();
                let cand = self.ws_focus_candidate(cur).filter(|c| visible.get(c).copied().unwrap_or(false));
                self.focused = None;
                self.set_focus(cand);
            }
        }
    }

    /// Tell river which window gets the keyboard. River has no event saying
    /// where keyboard focus really is, so the cached `focus_sent` is dropped
    /// whenever river may have moved focus on its own (layer-shell focus such
    /// as rofi, the lock screen) and when focusing a window that is only being
    /// un-hidden in this same sequence (river ignores focus for hidden windows,
    /// so it is sent again in the next sequence).
    fn apply_focus(&mut self) {
        let focused = self.focused;
        let just_shown = focused.is_some_and(|f| self.newly_shown.contains(&f));
        self.newly_shown.clear();
        for seat in &mut self.seats {
            if seat.layer_exclusive {
                seat.focus_sent = None;
                continue;
            }
            if seat.focus_sent == Some(focused) {
                continue;
            }
            match focused.and_then(|id| self.windows.get(&id)) {
                Some(w) => seat.proxy.focus_window(&w.proxy),
                None => seat.proxy.clear_focus(),
            }
            if just_shown {
                seat.focus_sent = None;
                self.refocus = true;
            } else {
                seat.focus_sent = Some(focused);
            }
        }
    }

    // ---- render sequence --------------------------------------------------

    fn render(&mut self, qh: &QueueHandle<State>) {
        // Floating windows waiting for their first size get centred now.
        for (ws, area) in self.float_areas() {
            for w in self.windows.values_mut() {
                if w.floating && !w.float_placed && w.ws == ws && w.dims.0 > 0 {
                    w.float = center(area, w.dims.0, w.dims.1);
                    w.float_placed = true;
                    w.target = Some((w.float.x, w.float.y));
                }
            }
        }
        for seat in &self.seats {
            if let Op::Move { win, start } = seat.op {
                if let Some(w) = self.windows.get_mut(&win) {
                    w.float.x = start.0 + seat.op_delta.0;
                    w.float.y = start.1 + seat.op_delta.1;
                    w.target = Some((w.float.x, w.float.y));
                }
            }
        }
        for w in self.windows.values_mut() {
            if w.floating {
                w.float.w = w.dims.0.max(1);
                w.float.h = w.dims.1.max(1);
            }
            if let (Some((x, y)), Some(true)) = (w.target, w.shown) {
                w.node.set_position(x, y);
            }
        }
        // Stacking order: floating above tiled, focused floating on top,
        // fullscreen above everything.
        let focused = self.focused;
        for (id, w) in &self.windows {
            if w.floating && Some(*id) != focused {
                w.node.place_top();
            }
        }
        if let Some(w) = focused.and_then(|f| self.windows.get(&f)) {
            if w.floating {
                w.node.place_top();
            }
        }
        for w in self.windows.values() {
            if w.fullscreen {
                w.node.place_top();
            }
        }
        self.render_bars(qh);
        if let Some(wm) = &self.wm {
            wm.render_finish();
        }
    }
}

impl State {
    fn title_text(&self, id: WinId, count: usize) -> String {
        let Some(w) = self.windows.get(&id) else { return String::new() };
        let t = self.config.general.title_format.replace("{title}", &w.title).replace("{app_id}", &w.app_id);
        if count > 1 { format!("{t} [{count}]") } else { t }
    }

    /// Turn the tree's title bars into drawable bar specs.
    fn build_bars(&mut self) {
        let mut bars = Vec::new();
        for o in &self.outputs {
            for tb in &o.title_bars {
                let Some(owner) = tb.tabs.iter().find(|t| t.active).map(|t| t.window) else { continue };
                let x0 = tb.tabs.iter().map(|t| t.rect.x).min().unwrap_or(0);
                let y0 = tb.tabs.iter().map(|t| t.rect.y).min().unwrap_or(0);
                let x1 = tb.tabs.iter().map(|t| t.rect.x + t.rect.w).max().unwrap_or(0);
                let y1 = tb.tabs.iter().map(|t| t.rect.y + t.rect.h).max().unwrap_or(0);
                let rect = Rect::new(x0, y0, x1 - x0, y1 - y0);
                if rect.w <= 0 || rect.h <= 0 {
                    continue;
                }
                let rows = tb
                    .tabs
                    .iter()
                    .map(|t| Row {
                        window: t.window,
                        rect: Rect::new(t.rect.x - x0, t.rect.y - y0, t.rect.w, t.rect.h),
                        text: self.title_text(t.window, t.count),
                        state: if Some(t.window) == self.focused {
                            TabState::Focused
                        } else if t.active {
                            TabState::Active
                        } else {
                            TabState::Inactive
                        },
                    })
                    .collect();
                bars.push(BarSpec { owner, rect, rows });
            }
        }
        self.bars = bars;
    }

    /// Attach, redraw and position title bar decorations for this frame.
    fn render_bars(&mut self, qh: &QueueHandle<State>) {
        let (Some(compositor), Some(shm)) = (self.compositor.clone(), self.shm.clone()) else { return };
        let bars = std::mem::take(&mut self.bars);
        let owners: Vec<WinId> = bars.iter().map(|b| b.owner).collect();
        for id in self.decos.keys().copied().filter(|id| !owners.contains(id)).collect::<Vec<_>>() {
            self.decos.remove(&id).unwrap().destroy();
        }
        let c = &self.config.colors;
        let pal = Palette {
            focused: (c.focused_title, c.focused_title_text),
            active: (c.active_title, c.active_title_text),
            inactive: (c.unfocused_title, c.unfocused_title_text),
            separator: c.title_separator,
        };
        for bar in &bars {
            let Some(w) = self.windows.get(&bar.owner) else { continue };
            let Some((tx, ty)) = w.target else { continue };
            let offset = (bar.rect.x - tx, bar.rect.y - ty);
            let deco = self.decos.entry(bar.owner).or_insert_with(|| {
                let surface = compositor.create_surface(qh, ());
                let deco = w.proxy.get_decoration_above(&surface, qh, ());
                Deco { surface, deco, last: None, offset: None }
            });
            if deco.offset != Some(offset) {
                deco.deco.set_offset(offset.0, offset.1);
                deco.offset = Some(offset);
            }
            if deco.last.as_ref() != Some(bar) {
                let text = self.text.get_or_insert_with(|| Text::new(&self.config.general.font));
                let px = deco::paint(text, bar, &pal);
                if let Some(buffer) = deco::upload(&shm, qh, bar.rect.w, bar.rect.h, &px) {
                    deco.deco.sync_next_commit();
                    deco.surface.attach(Some(&buffer), 0, 0);
                    deco.surface.damage_buffer(0, 0, bar.rect.w, bar.rect.h);
                    deco.surface.commit();
                    deco.last = Some(bar.clone());
                }
            }
        }
        self.bars = bars;
    }
}

impl State {
    /// Whether some window's `inhibit_idle` rule currently applies. River only
    /// honours idle inhibitors from client windows, not from WM surfaces, so
    /// chaophraya reports this in its state (`idle_inhibited`) and the
    /// idle-lock script waits instead of locking while it is true.
    fn update_idle_inhibit(&mut self) {
        let active = self.windows.iter().any(|(id, w)| {
            let visible = w.shown == Some(true);
            match w.inhibit_idle {
                None => false,
                Some(IdleMode::Open) => true,
                Some(IdleMode::Visible) => visible,
                Some(IdleMode::Focus) => self.focused == Some(*id),
                Some(IdleMode::Fullscreen) => visible && w.fullscreen,
            }
        });
        if active != self.idle_inhibited {
            self.idle_inhibited = active;
            self.state_changed = true;
        }
    }

    pub fn idle_inhibited(&self) -> bool {
        self.idle_inhibited
    }
}

impl State {
    /// The tab under the pointer, if it is over a title bar.
    fn tab_under_pointer(&self) -> Option<WinId> {
        let (surface, x, y) = self.pointer_on.clone()?;
        let bar = self.decos.values().find(|d| d.surface.id() == surface)?.last.as_ref()?;
        let (x, y) = (x as i32, y as i32);
        bar.rows
            .iter()
            .find(|r| x >= r.rect.x && x < r.rect.x + r.rect.w && y >= r.rect.y && y < r.rect.y + r.rect.h)
            .map(|r| r.window)
    }

    /// Switch tabs when the left button is released over the same tab it was
    /// pressed on. (Switching on press would destroy the title-bar surface
    /// while the button is still held on it, which breaks the pointer grab.)
    fn title_release(&mut self) {
        let pressed = self.tab_pressed.take();
        if let Some(w) = pressed.filter(|w| self.tab_under_pointer() == Some(*w)) {
            self.queue.push(Pending { command: Command::FocusId(w), client: None });
            if let Some(wm) = &self.wm {
                wm.manage_dirty();
            }
        }
    }
}

/// An output that shows a workspace: present, not mirroring, not parked.
fn usable(o: &Output) -> bool {
    !o.removed && !o.mirror && !o.parked
}

fn center(area: Rect, w: i32, h: i32) -> Rect {
    Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h)
}

// ---- Wayland dispatch -----------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            state.registry = Some(registry.clone());
            match interface.as_str() {
                "river_window_manager_v1" if version >= 4 => {
                    state.wm = Some(registry.bind(name, 4, qh, ()));
                }
                "river_window_manager_v1" => {
                    state.exit = Some(format!("river_window_manager_v1 v{version} is too old (need 4)"));
                }
                "river_xkb_bindings_v1" => state.xkb = Some(registry.bind(name, 1, qh, ())),
                "river_layer_shell_v1" => state.layer_shell = Some(registry.bind(name, 1, qh, ())),
                "wl_compositor" if version >= 4 => state.compositor = Some(registry.bind(name, 4, qh, ())),
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "wl_seat" => {
                    let _s: wl_seat::WlSeat = registry.bind(name, version.min(7), qh, ());
                }
                "wp_cursor_shape_manager_v1" => state.cursor_mgr = Some(registry.bind(name, 1, qh, ())),
                "river_input_manager_v1" => state.input.manager = Some(registry.bind(name, version.min(2), qh, ())),
                "river_xkb_config_v1" => state.input.xkb = Some(registry.bind(name, version.min(3), qh, ())),
                "river_libinput_config_v1" => {
                    state.input.libinput = Some(registry.bind(name, version.min(2), qh, ()))
                }
                "wl_output" => {
                    let _o: wl_output::WlOutput = registry.bind(name, version.min(4), qh, name);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.output_names.insert(*global, name);
            state.state_changed = true;
        }
    }
}

impl Dispatch<RiverWindowManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &RiverWindowManagerV1,
        event: river_window_manager_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use river_window_manager_v1::Event;
        match event {
            Event::Unavailable => state.exit = Some("another window manager is already running".into()),
            Event::Finished => state.exit = Some(String::new()),
            Event::ManageStart => state.manage(qh),
            Event::SessionLocked | Event::SessionUnlocked => {
                state.session_locked = matches!(event, Event::SessionLocked);
                // The lock screen took the keyboard; give it back on unlock.
                for seat in &mut state.seats {
                    seat.focus_sent = None;
                }
                if let Some(wm) = &state.wm {
                    wm.manage_dirty();
                }
            }
            Event::RenderStart => state.render(qh),
            Event::Window { id } => {
                let node = id.get_node(qh, ());
                let wid = state.next_id;
                state.next_id += 1;
                state.by_proxy.insert(id.id(), wid);
                state.windows.insert(
                    wid,
                    Window {
                        proxy: id,
                        node,
                        app_id: String::new(),
                        title: String::new(),
                        has_parent: false,
                        fixed_size: false,
                        ws: 1,
                        floating: false,
                        sticky: false,
                        scratch: false,
                        fullscreen: false,
                        urgent: false,
                        float: Rect::default(),
                        float_placed: false,
                        float_size: None,
                        inhibit_idle: None,
                        border_width: None,
                        dims: (0, 0),
                        new: true,
                        closed: false,
                        proposed: None,
                        shown: None,
                        tiled_edges_sent: None,
                        target: None,
                        last_cell: Rect::default(),
                        fullscreen_req: None,
                        exit_fullscreen_req: false,
                        move_req: None,
                        resize_req: None,
                    },
                );
            }
            Event::Output { id } => {
                let layer = state.layer_shell.as_ref().map(|ls| ls.get_output(&id, qh, ()));
                state.outputs.push(Output {
                    proxy: id,
                    layer,
                    wl_name: None,
                    rect: Rect::default(),
                    area: None,
                    ws: 0,
                    removed: false,
                    mirror: false,
                    parked: false,
                    park_decided: false,
                    title_bars: Vec::new(),
                    view: Vec::new(),
                });
            }
            Event::Seat { id } => {
                let layer = state.layer_shell.as_ref().map(|ls| ls.get_seat(&id, qh, ()));
                state.seats.push(Seat {
                    proxy: id,
                    layer,
                    new_bindings: true,
                    removed: false,
                    xkb: Vec::new(),
                    xkb_meta: Vec::new(),
                    bindings_state: None,
                    pointer: Vec::new(),
                    hovered: None,
                    hover_changed: false,
                    pointer_pos: None,
                    pointer_moved: false,
                    interacted: None,
                    layer_exclusive: false,
                    op: Op::None,
                    op_delta: (0, 0),
                    op_release: false,
                    ptr_action: None,
                    focus_sent: None,
                });
            }
        }
    }

    wayland_client::event_created_child!(State, RiverWindowManagerV1, [
        river_window_manager_v1::EVT_WINDOW_OPCODE => (RiverWindowV1, ()),
        river_window_manager_v1::EVT_OUTPUT_OPCODE => (RiverOutputV1, ()),
        river_window_manager_v1::EVT_SEAT_OPCODE => (RiverSeatV1, ())
    ]);
}

impl Dispatch<RiverWindowV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverWindowV1,
        event: river_window_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_window_v1::Event;
        let Some(id) = state.by_proxy.get(&proxy.id()).copied() else { return };
        let focused = state.focused == Some(id);
        let Some(w) = state.windows.get_mut(&id) else { return };
        match event {
            Event::Closed => w.closed = true,
            Event::Dimensions { width, height } => w.dims = (width, height),
            Event::DimensionsHint { min_width, min_height, max_width, max_height } => {
                w.fixed_size = min_width > 0 && min_width == max_width && min_height > 0 && min_height == max_height;
            }
            Event::AppId { app_id } => {
                w.app_id = app_id.unwrap_or_default();
                state.state_changed |= focused;
            }
            Event::Title { title } => {
                w.title = title.unwrap_or_default();
                state.state_changed |= focused;
            }
            Event::Parent { parent } => w.has_parent = parent.is_some(),
            Event::FullscreenRequested { output } => w.fullscreen_req = Some(output),
            Event::ExitFullscreenRequested => w.exit_fullscreen_req = true,
            Event::PointerMoveRequested { seat } => w.move_req = Some(seat),
            Event::PointerResizeRequested { seat, edges } => {
                if let Ok(e) = edges.into_result() {
                    w.resize_req = Some((seat, e));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverOutputV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverOutputV1,
        event: river_output_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_output_v1::Event;
        let Some(o) = state.outputs.iter_mut().find(|o| &o.proxy == proxy) else { return };
        match event {
            Event::Removed => o.removed = true,
            Event::WlOutput { name } => o.wl_name = Some(name),
            Event::Position { x, y } => {
                o.rect.x = x;
                o.rect.y = y;
            }
            Event::Dimensions { width, height } => {
                o.rect.w = width;
                o.rect.h = height;
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverLayerShellOutputV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverLayerShellOutputV1,
        event: river_layer_shell_output_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        #[allow(irrefutable_let_patterns)]
        if let river_layer_shell_output_v1::Event::NonExclusiveArea { x, y, width, height } = event {
            if let Some(o) = state.outputs.iter_mut().find(|o| o.layer.as_ref() == Some(proxy)) {
                o.area = Some(Rect::new(x, y, width, height));
            }
        }
    }
}

impl Dispatch<RiverLayerShellSeatV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverLayerShellSeatV1,
        event: river_layer_shell_seat_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_layer_shell_seat_v1::Event;
        let Some(s) = state.seats.iter_mut().find(|s| s.layer.as_ref() == Some(proxy)) else { return };
        match event {
            Event::FocusExclusive => s.layer_exclusive = true,
            Event::FocusNonExclusive | Event::FocusNone => {
                // A layer surface (e.g. rofi) let go of the keyboard: hand it
                // back to the focused window in a new manage sequence.
                s.layer_exclusive = false;
                s.focus_sent = None;
                if let Some(wm) = &state.wm {
                    wm.manage_dirty();
                }
            }
        }
    }
}

impl Dispatch<RiverSeatV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverSeatV1,
        event: river_seat_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_seat_v1::Event;
        let win = |w: &RiverWindowV1| state.by_proxy.get(&w.id()).copied();
        let ids = match &event {
            Event::PointerEnter { window } | Event::WindowInteraction { window } => win(window),
            _ => None,
        };
        let Some(s) = state.seats.iter_mut().find(|s| &s.proxy == proxy) else { return };
        match event {
            Event::Removed => s.removed = true,
            Event::PointerEnter { .. } => {
                s.hovered = ids;
                s.hover_changed = true;
            }
            Event::PointerLeave => s.hovered = None,
            Event::WindowInteraction { .. } => s.interacted = ids,
            Event::OpDelta { dx, dy } => s.op_delta = (dx, dy),
            Event::OpRelease => s.op_release = true,
            Event::PointerPosition { x, y } => {
                if s.pointer_pos.is_some_and(|p| p != (x, y)) {
                    s.pointer_moved = true;
                }
                s.pointer_pos = Some((x, y));
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverXkbBindingV1, ObjectId> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverXkbBindingV1,
        event: river_xkb_binding_v1::Event,
        seat_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_xkb_binding_v1::Event::Pressed = event {
            let Some(s) = state.seats.iter().find(|s| &s.proxy.id() == seat_id) else { return };
            if let Some((_, cmd)) = s.xkb.iter().find(|(b, _)| b == proxy) {
                let cmd = cmd.clone();
                state.binding_actions.entry(seat_id.clone()).or_default().push(cmd);
            }
        }
    }
}

impl Dispatch<RiverPointerBindingV1, ObjectId> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverPointerBindingV1,
        event: river_pointer_binding_v1::Event,
        seat_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_pointer_binding_v1::Event::Pressed = event {
            let Some(s) = state.seats.iter_mut().find(|s| &s.proxy.id() == seat_id) else { return };
            if let Some(i) = s.pointer.iter().position(|b| b == proxy) {
                s.ptr_action = Some(if i == 0 { BTN_LEFT } else { BTN_RIGHT });
            }
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(_: &mut Self, buffer: &wl_buffer::WlBuffer, event: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

wayland_client::delegate_noop!(State: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(State: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(_: &mut Self, s: &wl_surface::WlSurface, e: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if std::env::var_os("CHAOPHRAYA_DEBUG").is_some() {
            eprintln!("chaophraya: surface {:?} {e:?}", s.id());
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointers.is_empty() {
                let pointer = seat.get_pointer(qh, ());
                let shape = state.cursor_mgr.as_ref().map(|m| m.get_pointer(&pointer, qh, ()));
                state.pointers.push((pointer, shape));
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(state: &mut Self, pointer: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        const BTN_LEFT: u32 = 0x110;
        if std::env::var_os("CHAOPHRAYA_DEBUG").is_some() {
            let decos: Vec<_> = state.decos.iter().map(|(w, d)| (*w, d.surface.id().protocol_id())).collect();
            eprintln!("chaophraya: pointer {event:?} | on={:?} decos={decos:?}", state.pointer_on.as_ref().map(|p| p.0.protocol_id()));
        }
        match event {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                state.pointer_on = Some((surface.id(), surface_x, surface_y));
                // Our own surfaces need a cursor, or it vanishes over title bars.
                if let Some((_, Some(shape))) = state.pointers.iter().find(|(p, _)| p == pointer) {
                    shape.set_shape(serial, wp_cursor_shape_device_v1::Shape::Default);
                }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if let Some(p) = &mut state.pointer_on {
                    p.1 = surface_x;
                    p.2 = surface_y;
                }
            }
            // Only forget the surface we're tracking: when a title bar moves to
            // another window its old surface may report `leave` after the new
            // one reported `enter`.
            wl_pointer::Event::Leave { surface, .. } => {
                if state.pointer_on.as_ref().is_some_and(|(id, _, _)| *id == surface.id()) {
                    state.pointer_on = None;
                }
            }
            wl_pointer::Event::Button { button: BTN_LEFT, state: WEnum::Value(s), .. } => match s {
                wl_pointer::ButtonState::Pressed => state.tab_pressed = state.tab_under_pointer(),
                wl_pointer::ButtonState::Released => state.title_release(),
                _ => {}
            },
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore WpCursorShapeManagerV1);
wayland_client::delegate_noop!(State: ignore WpCursorShapeDeviceV1);
wayland_client::delegate_noop!(State: ignore RiverDecorationV1);
wayland_client::delegate_noop!(State: ignore RiverXkbBindingsV1);
wayland_client::delegate_noop!(State: ignore RiverNodeV1);
wayland_client::delegate_noop!(State: ignore RiverLayerShellV1);
