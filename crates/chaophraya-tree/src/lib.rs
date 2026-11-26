//! Sway-style tiling tree, independent of any compositor.
//!
//! One [`Tree`] holds the tiled windows of one tag. Windows are identified by
//! any small copyable id `W`. The tree decides where new windows go, how focus
//! and windows move between neighbours, and turns itself into rectangles with
//! [`Tree::arrange`] (including the title-bar areas of stacking/tabbed
//! containers).
//!
//! Insertion rule: a lone window is split left-right; after that a new window
//! splits the focused window top-bottom (or joins its container if that is
//! already vertical, stacking or tabbed, or a fresh explicit split).

use std::fmt::Debug;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w: w.max(0), h: h.max(0) }
    }

    /// Shrink by `d` on every side (never below zero size).
    pub fn inset(self, d: i32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2 * d, self.h - 2 * d)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    SplitH,
    SplitV,
    Stacking,
    Tabbed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    H,
    V,
}

impl Layout {
    /// The axis along which children follow each other (also the axis that
    /// focus/move commands travel along inside this container).
    pub fn axis(self) -> Axis {
        match self {
            Layout::SplitH | Layout::Tabbed => Axis::H,
            Layout::SplitV | Layout::Stacking => Axis::V,
        }
    }

    fn split_for(axis: Axis) -> Layout {
        match axis {
            Axis::H => Layout::SplitH,
            Axis::V => Layout::SplitV,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::H,
            Dir::Up | Dir::Down => Axis::V,
        }
    }

    fn forward(self) -> bool {
        matches!(self, Dir::Right | Dir::Down)
    }
}

type NodeId = usize;

#[derive(Clone, Debug)]
enum Kind<W> {
    Leaf(W),
    Container {
        layout: Layout,
        /// Split layout to return to from stacking/tabbed with "toggle split".
        last_split: Layout,
        children: Vec<NodeId>,
        /// Index of the most recently focused child.
        focus: usize,
    },
}

#[derive(Clone, Debug)]
struct Node<W> {
    kind: Kind<W>,
    parent: Option<NodeId>,
    /// Relative size among siblings.
    weight: f64,
    /// Rectangle from the last `arrange`, used to turn pixel resizes into weights.
    rect: Rect,
}

/// A title bar drawn above a stacking or tabbed container.
#[derive(Clone, Debug, PartialEq)]
pub struct TitleBar<W> {
    pub layout: Layout,
    pub tabs: Vec<Tab<W>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab<W> {
    /// The window whose title the tab shows (for a nested container: its
    /// focused window).
    pub window: W,
    /// Number of windows behind this tab (>1 for a nested container).
    pub count: usize,
    pub rect: Rect,
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrangement<W> {
    /// Visible windows and the cell each occupies (borders go inside the cell).
    pub windows: Vec<(W, Rect)>,
    /// Windows hidden behind a stacking/tabbed container's active child.
    pub hidden: Vec<W>,
    pub title_bars: Vec<TitleBar<W>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Style {
    /// Gap between neighbouring cells.
    pub gap: i32,
    /// Height of one title row in stacking/tabbed containers.
    pub title_height: i32,
}

#[derive(Clone, Debug)]
pub struct Tree<W> {
    nodes: Vec<Option<Node<W>>>,
    free: Vec<NodeId>,
    /// Always a container when present.
    root: Option<NodeId>,
    /// Gap used by the last `arrange` (resizes are relative to the space
    /// left after gaps).
    last_gap: i32,
}

impl<W: Copy + Eq + Debug> Default for Tree<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Copy + Eq + Debug> Tree<W> {
    pub fn new() -> Self {
        Tree { nodes: Vec::new(), free: Vec::new(), root: None, last_gap: 0 }
    }

    // ---- arena helpers ----------------------------------------------------

    fn alloc(&mut self, kind: Kind<W>, parent: Option<NodeId>, weight: f64) -> NodeId {
        let node = Some(Node { kind, parent, weight, rect: Rect::default() });
        match self.free.pop() {
            Some(id) => {
                self.nodes[id] = node;
                id
            }
            None => {
                self.nodes.push(node);
                self.nodes.len() - 1
            }
        }
    }

    fn dealloc(&mut self, id: NodeId) {
        self.nodes[id] = None;
        self.free.push(id);
    }

    fn n(&self, id: NodeId) -> &Node<W> {
        self.nodes[id].as_ref().expect("dangling node id")
    }

    fn n_mut(&mut self, id: NodeId) -> &mut Node<W> {
        self.nodes[id].as_mut().expect("dangling node id")
    }

    fn container(layout: Layout, children: Vec<NodeId>) -> Kind<W> {
        let last_split = if matches!(layout, Layout::SplitH | Layout::SplitV) { layout } else { Layout::SplitH };
        Kind::Container { layout, last_split, children, focus: 0 }
    }

    fn children(&self, id: NodeId) -> &[NodeId] {
        match &self.n(id).kind {
            Kind::Container { children, .. } => children,
            Kind::Leaf(_) => &[],
        }
    }

    fn children_mut(&mut self, id: NodeId) -> &mut Vec<NodeId> {
        match &mut self.n_mut(id).kind {
            Kind::Container { children, .. } => children,
            Kind::Leaf(_) => panic!("leaf has no children"),
        }
    }

    fn layout_of(&self, id: NodeId) -> Option<Layout> {
        match &self.n(id).kind {
            Kind::Container { layout, .. } => Some(*layout),
            Kind::Leaf(_) => None,
        }
    }

    fn is_leaf(&self, id: NodeId) -> bool {
        matches!(self.n(id).kind, Kind::Leaf(_))
    }

    fn index_in_parent(&self, id: NodeId) -> Option<(NodeId, usize)> {
        let p = self.n(id).parent?;
        let i = self.children(p).iter().position(|&c| c == id).expect("child not in parent");
        Some((p, i))
    }

    fn leaves_under(&self, id: NodeId, out: &mut Vec<W>) {
        match &self.n(id).kind {
            Kind::Leaf(w) => out.push(*w),
            Kind::Container { children, .. } => {
                for &c in children {
                    self.leaves_under(c, out);
                }
            }
        }
    }

    fn find(&self, w: W) -> Option<NodeId> {
        self.nodes.iter().position(|n| matches!(n, Some(Node { kind: Kind::Leaf(x), .. }) if *x == w))
    }

    /// Follow each container's focused child down to a leaf.
    fn focus_leaf_under(&self, mut id: NodeId) -> NodeId {
        while let Kind::Container { children, focus, .. } = &self.n(id).kind {
            match children.get(*focus).or(children.last()) {
                Some(&c) => id = c,
                None => break,
            }
        }
        id
    }

    fn leaf_window(&self, id: NodeId) -> Option<W> {
        match self.n(id).kind {
            Kind::Leaf(w) => Some(w),
            Kind::Container { .. } => None,
        }
    }

    // ---- queries ----------------------------------------------------------

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn len(&self) -> usize {
        self.windows().len()
    }

    pub fn contains(&self, w: W) -> bool {
        self.find(w).is_some()
    }

    /// All windows in tree order (left-to-right, top-to-bottom).
    pub fn windows(&self) -> Vec<W> {
        let mut out = Vec::new();
        if let Some(r) = self.root {
            self.leaves_under(r, &mut out);
        }
        out
    }

    /// The window that has focus within this tree.
    pub fn focused(&self) -> Option<W> {
        let leaf = self.focus_leaf_under(self.root?);
        self.leaf_window(leaf)
    }

    /// Layout of the container directly holding `w`.
    pub fn parent_layout(&self, w: W) -> Option<Layout> {
        let p = self.n(self.find(w)?).parent?;
        self.layout_of(p)
    }

    // ---- focus ------------------------------------------------------------

    /// Make `w` the focused window of the tree. Returns false if absent.
    pub fn set_focus(&mut self, w: W) -> bool {
        let Some(mut id) = self.find(w) else { return false };
        while let Some((p, i)) = self.index_in_parent(id) {
            if let Kind::Container { focus, .. } = &mut self.n_mut(p).kind {
                *focus = i;
            }
            id = p;
        }
        true
    }

    /// Move focus to the neighbour in `dir`, sway style. Returns the newly
    /// focused window, or None at the edge.
    pub fn focus_dir(&mut self, dir: Dir) -> Option<W> {
        let mut cur = self.focus_leaf_under(self.root?);
        while let Some((p, i)) = self.index_in_parent(cur) {
            if self.layout_of(p).map(Layout::axis) == Some(dir.axis()) {
                let n = self.children(p).len();
                let next = if dir.forward() { i + 1 } else { i.wrapping_sub(1) };
                if next < n {
                    let target = self.focus_leaf_under(self.children(p)[next]);
                    let w = self.leaf_window(target)?;
                    self.set_focus(w);
                    return Some(w);
                }
            }
            cur = p;
        }
        None
    }

    // ---- insertion / removal ----------------------------------------------

    fn mean_weight(&self, p: NodeId) -> f64 {
        let ch = self.children(p);
        if ch.is_empty() {
            1.0
        } else {
            ch.iter().map(|&c| self.n(c).weight).sum::<f64>() / ch.len() as f64
        }
    }

    /// Insert `leaf` into container `p` at position `at`, sized like an
    /// average sibling.
    fn attach(&mut self, p: NodeId, at: usize, leaf: NodeId) {
        let weight = self.mean_weight(p);
        let node = self.n_mut(leaf);
        node.parent = Some(p);
        node.weight = weight;
        if let Kind::Container { focus, children, .. } = &mut self.n_mut(p).kind {
            let at = at.min(children.len());
            // Keep the focus index pointing at the same child.
            if !children.is_empty() && *focus >= at {
                *focus += 1;
            }
            children.insert(at, leaf);
        }
    }

    /// Replace node `old` (in its parent) with a new container of `layout`
    /// holding `old`. Returns the container.
    fn wrap(&mut self, old: NodeId, layout: Layout) -> NodeId {
        let (p, i) = self.index_in_parent(old).expect("cannot wrap the root");
        let weight = self.n(old).weight;
        let c = self.alloc(Self::container(layout, vec![old]), Some(p), weight);
        self.children_mut(p)[i] = c;
        let node = self.n_mut(old);
        node.parent = Some(c);
        node.weight = 1.0;
        c
    }

    /// Add a new window using the insertion rule and focus it.
    pub fn insert(&mut self, w: W) {
        debug_assert!(!self.contains(w), "window inserted twice");
        let leaf = self.alloc(Kind::Leaf(w), None, 1.0);
        let Some(root) = self.root else {
            let r = self.alloc(Self::container(Layout::SplitH, vec![]), None, 1.0);
            self.root = Some(r);
            self.attach(r, 0, leaf);
            self.set_focus(w);
            return;
        };
        let target = self.focus_leaf_under(root);
        let (p, i) = self.index_in_parent(target).expect("leaf without parent");
        let joins_parent = self.children(p).len() == 1
            || matches!(self.layout_of(p), Some(Layout::SplitV | Layout::Stacking | Layout::Tabbed));
        if joins_parent {
            // Lone window under the root: the root is the left-right split.
            if p == root && self.children(p).len() == 1 && self.root_is_default(root) {
                self.set_layout_raw(root, Layout::SplitH);
            }
            self.attach(p, i + 1, leaf);
        } else {
            let c = self.wrap(target, Layout::SplitV);
            self.attach(c, 1, leaf);
        }
        self.set_focus(w);
    }

    /// The root has not been given an explicit layout by the user while it
    /// held a single window (see `split`).
    fn root_is_default(&self, root: NodeId) -> bool {
        !matches!(self.layout_of(root), Some(Layout::SplitV | Layout::Stacking | Layout::Tabbed))
            || self.children(root).len() != 1
    }

    /// Add a sticky window at its fixed slot: the bottom of the right-most
    /// column. Does not change focus.
    pub fn insert_sticky(&mut self, w: W) {
        debug_assert!(!self.contains(w), "window inserted twice");
        let focus_before = self.focused();
        let leaf = self.alloc(Kind::Leaf(w), None, 1.0);
        let Some(root) = self.root else {
            let r = self.alloc(Self::container(Layout::SplitH, vec![]), None, 1.0);
            self.root = Some(r);
            self.attach(r, 0, leaf);
            return;
        };
        if self.len() == 1 && self.children(root).len() == 1 {
            self.set_layout_raw(root, Layout::SplitH);
            self.attach(root, 1, leaf);
        } else {
            let mut col = root;
            while self.layout_of(col) == Some(Layout::SplitH) {
                col = *self.children(col).last().expect("empty container");
            }
            if self.layout_of(col) == Some(Layout::SplitV) {
                let n = self.children(col).len();
                self.attach(col, n, leaf);
            } else if col == root {
                // Root is stacking/tabbed: put the sticky window beneath it.
                self.wrap_root(Layout::SplitV);
                self.attach(root, 1, leaf);
            } else {
                let c = self.wrap(col, Layout::SplitV);
                self.attach(c, 1, leaf);
            }
        }
        if let Some(f) = focus_before {
            self.set_focus(f);
        }
    }

    fn set_layout_raw(&mut self, id: NodeId, l: Layout) {
        if let Kind::Container { layout, last_split, .. } = &mut self.n_mut(id).kind {
            *layout = l;
            if matches!(l, Layout::SplitH | Layout::SplitV) {
                *last_split = l;
            }
        }
    }

    /// Move the root's contents into a new child container and give the root
    /// `layout`, keeping the root id stable.
    fn wrap_root(&mut self, layout: Layout) {
        let root = self.root.expect("no root");
        let kind = std::mem::replace(&mut self.n_mut(root).kind, Self::container(layout, vec![]));
        let c = self.alloc(kind, Some(root), 1.0);
        for ch in self.children(c).to_vec() {
            self.n_mut(ch).parent = Some(c);
        }
        if let Kind::Container { children, .. } = &mut self.n_mut(root).kind {
            children.push(c);
        }
    }

    /// Detach `id` from its parent without cleaning up the parent.
    fn detach_raw(&mut self, id: NodeId) -> NodeId {
        let (p, i) = self.index_in_parent(id).expect("detach root");
        self.children_mut(p).remove(i);
        if let Kind::Container { focus, children, .. } = &mut self.n_mut(p).kind {
            if *focus > i || (*focus == i && *focus >= children.len()) {
                *focus = focus.saturating_sub(1);
            }
        }
        self.n_mut(id).parent = None;
        p
    }

    /// Clean up container `id` after a child left: drop empty containers and
    /// replace single-child containers by their child.
    fn normalize(&mut self, id: NodeId) {
        if self.nodes[id].is_none() {
            return;
        }
        let n = self.children(id).len();
        match self.n(id).parent {
            Some(_) if n == 0 => {
                let p = self.detach_raw(id);
                self.dealloc(id);
                self.normalize(p);
            }
            Some(_) if n == 1 => {
                let child = self.children(id)[0];
                let (p, i) = self.index_in_parent(id).unwrap();
                let weight = self.n(id).weight;
                self.children_mut(p)[i] = child;
                let c = self.n_mut(child);
                c.parent = Some(p);
                c.weight = weight;
                self.dealloc(id);
            }
            Some(_) => {}
            None if n == 0 => {
                self.dealloc(id);
                self.root = None;
            }
            None if n == 1 => {
                let child = self.children(id)[0];
                if !self.is_leaf(child) {
                    // Hoist the only child container into the root.
                    let kind = self.nodes[child].take().expect("dangling node id").kind;
                    self.free.push(child);
                    self.n_mut(id).kind = kind;
                    for ch in self.children(id).to_vec() {
                        self.n_mut(ch).parent = Some(id);
                    }
                    self.normalize(id);
                } else {
                    // A lone window: back to the default left-right root.
                    self.set_layout_raw(id, Layout::SplitH);
                    if let Kind::Container { focus, .. } = &mut self.n_mut(id).kind {
                        *focus = 0;
                    }
                }
            }
            None => {}
        }
    }

    /// Remove a window. Returns false if it was not in the tree.
    pub fn remove(&mut self, w: W) -> bool {
        let Some(id) = self.find(w) else { return false };
        let p = self.detach_raw(id);
        self.dealloc(id);
        self.normalize(p);
        true
    }

    // ---- moving -----------------------------------------------------------

    /// Move window `w` one step in `dir`, sway style: swap with a neighbouring
    /// window, enter a neighbouring container, or leave the current container.
    /// Returns false if nothing changed.
    pub fn move_dir(&mut self, w: W, dir: Dir) -> bool {
        let Some(node) = self.find(w) else { return false };
        let mut cur = node;
        loop {
            let Some((p, i)) = self.index_in_parent(cur) else {
                // `cur` is the root. Either the window already sits at the
                // edge of a root split along `dir`, or no ancestor runs along
                // `dir` and the window goes beside everything else.
                let at_edge = self.n(node).parent == Some(cur)
                    && self.layout_of(cur).map(Layout::axis) == Some(dir.axis());
                if at_edge || self.len() < 2 {
                    return false;
                }
                let root = cur;
                self.wrap_root(Layout::split_for(dir.axis()));
                let old = self.detach_raw(node);
                let at = if dir.forward() { 1 } else { 0 };
                self.attach(root, at, node);
                self.normalize(old);
                self.set_focus(w);
                return true;
            };
            if self.layout_of(p).map(Layout::axis) == Some(dir.axis()) {
                let n = self.children(p).len();
                let next = if dir.forward() { i + 1 } else { i.wrapping_sub(1) };
                if cur == node {
                    if next < n {
                        let sib = self.children(p)[next];
                        if self.is_leaf(sib) {
                            self.children_mut(p).swap(i, next);
                        } else {
                            let old = self.detach_raw(node);
                            let at = if self.layout_of(sib).map(Layout::axis) == Some(dir.axis()) {
                                if dir.forward() { 0 } else { self.children(sib).len() }
                            } else {
                                self.focus_index(sib) + 1
                            };
                            self.attach(sib, at, node);
                            self.normalize(old);
                        }
                        self.set_focus(w);
                        return true;
                    }
                } else {
                    // Leave the sub-container `cur` and sit beside it in `p`.
                    // Attach before cleaning up so `p` cannot be flattened away.
                    let old = self.detach_raw(node);
                    let at = if dir.forward() { i + 1 } else { i };
                    self.attach(p, at, node);
                    self.normalize(old);
                    self.set_focus(w);
                    return true;
                }
            }
            cur = p;
        }
    }

    fn focus_index(&self, id: NodeId) -> usize {
        match &self.n(id).kind {
            Kind::Container { focus, children, .. } => (*focus).min(children.len().saturating_sub(1)),
            Kind::Leaf(_) => 0,
        }
    }

    // ---- layout changes ---------------------------------------------------

    /// Set the layout of the container holding `w` (sway `layout ...`).
    pub fn set_layout(&mut self, w: W, layout: Layout) -> bool {
        let Some(id) = self.find(w) else { return false };
        let Some(p) = self.n(id).parent else { return false };
        self.set_layout_raw(p, layout);
        true
    }

    /// Sway `layout toggle split`: swap splith/splitv, or return from
    /// stacking/tabbed to the previous split.
    pub fn toggle_split(&mut self, w: W) -> bool {
        let Some(id) = self.find(w) else { return false };
        let Some(p) = self.n(id).parent else { return false };
        let next = match &self.n(p).kind {
            Kind::Container { layout: Layout::SplitH, .. } => Layout::SplitV,
            Kind::Container { layout: Layout::SplitV, .. } => Layout::SplitH,
            Kind::Container { last_split, .. } => *last_split,
            Kind::Leaf(_) => return false,
        };
        self.set_layout_raw(p, next);
        true
    }

    /// Sway `splith`/`splitv`: the next window opened beside `w` goes in
    /// direction `axis`.
    pub fn split(&mut self, w: W, axis: Axis) -> bool {
        let Some(id) = self.find(w) else { return false };
        let Some(p) = self.n(id).parent else { return false };
        let layout = Layout::split_for(axis);
        if self.children(p).len() == 1 {
            self.set_layout_raw(p, layout);
        } else {
            self.wrap(id, layout);
        }
        true
    }

    /// Grow (positive) or shrink (negative) `w` by `px` along `axis`, using the
    /// nearest container that runs along that axis. Needs a prior `arrange`.
    pub fn resize(&mut self, w: W, axis: Axis, px: i32) -> bool {
        let Some(mut cur) = self.find(w) else { return false };
        while let Some((p, i)) = self.index_in_parent(cur) {
            let is_split = matches!(self.layout_of(p), Some(Layout::SplitH | Layout::SplitV));
            if is_split && self.layout_of(p).map(Layout::axis) == Some(axis) && self.children(p).len() > 1 {
                let gaps = self.last_gap * (self.children(p).len() as i32 - 1);
                let extent = match axis {
                    Axis::H => self.n(p).rect.w,
                    Axis::V => self.n(p).rect.h,
                } - gaps;
                if extent <= 0 {
                    return false;
                }
                let children = self.children(p).to_vec();
                let total: f64 = children.iter().map(|&c| self.n(c).weight).sum();
                let share = self.n(children[i]).weight / total;
                let new_share = (share + px as f64 / extent as f64).clamp(0.05, 0.95);
                let others = 1.0 - share;
                for (k, &c) in children.iter().enumerate() {
                    let s = self.n(c).weight / total;
                    let ns = if k == i { new_share } else if others > 0.0 { s / others * (1.0 - new_share) } else { 0.0 };
                    self.n_mut(c).weight = ns.max(0.01);
                }
                return true;
            }
            cur = p;
        }
        false
    }

    /// Move one edge of `w` by `px` (positive = towards `edge`, i.e. the window
    /// grows), trading size only with the neighbour on that edge, like
    /// dragging a split in sway. Uses the nearest ancestor that has a neighbour
    /// on that side. Needs a prior `arrange`. Returns false if there is none.
    pub fn resize_edge(&mut self, w: W, edge: Dir, px: i32) -> bool {
        let Some(mut cur) = self.find(w) else { return false };
        while let Some((p, i)) = self.index_in_parent(cur) {
            let layout = self.layout_of(p);
            let is_split = matches!(layout, Some(Layout::SplitH | Layout::SplitV));
            let n = self.children(p).len();
            let j = if edge.forward() { i + 1 } else { i.wrapping_sub(1) };
            if is_split && layout.map(Layout::axis) == Some(edge.axis()) && j < n {
                let gaps = self.last_gap * (n as i32 - 1);
                let extent = match edge.axis() {
                    Axis::H => self.n(p).rect.w,
                    Axis::V => self.n(p).rect.h,
                } - gaps;
                if extent <= 0 {
                    return false;
                }
                let children = self.children(p).to_vec();
                let total: f64 = children.iter().map(|&c| self.n(c).weight).sum();
                let (a, b) = (children[i], children[j]);
                let min = total * 0.05;
                let want = px as f64 / extent as f64 * total;
                // Keep both at least 5% of the container.
                let delta = want.clamp(min - self.n(a).weight, self.n(b).weight - min);
                self.n_mut(a).weight += delta;
                self.n_mut(b).weight -= delta;
                return true;
            }
            cur = p;
        }
        false
    }

    // ---- arrangement ------------------------------------------------------

    /// Compute cells for every window inside `area`.
    pub fn arrange(&mut self, area: Rect, style: Style) -> Arrangement<W> {
        let mut out = Arrangement { windows: Vec::new(), hidden: Vec::new(), title_bars: Vec::new() };
        self.last_gap = style.gap;
        if let Some(r) = self.root {
            self.arrange_node(r, area, style, &mut out);
        }
        out
    }

    fn arrange_node(&mut self, id: NodeId, rect: Rect, style: Style, out: &mut Arrangement<W>) {
        self.n_mut(id).rect = rect;
        let (layout, children, focus) = match &self.n(id).kind {
            Kind::Leaf(w) => {
                out.windows.push((*w, rect));
                return;
            }
            Kind::Container { layout, children, focus, .. } => (*layout, children.clone(), *focus),
        };
        if children.is_empty() {
            return;
        }
        let active = focus.min(children.len() - 1);
        match layout {
            Layout::SplitH | Layout::SplitV => {
                let horizontal = layout == Layout::SplitH;
                let extent = if horizontal { rect.w } else { rect.h };
                let gaps = style.gap * (children.len() as i32 - 1);
                let avail = (extent - gaps).max(0);
                let total: f64 = children.iter().map(|&c| self.n(c).weight).sum();
                let mut offset = 0;
                for (k, &c) in children.iter().enumerate() {
                    let len = if k + 1 == children.len() {
                        avail - offset
                    } else {
                        ((avail as f64) * self.n(c).weight / total).round() as i32
                    };
                    let len = len.clamp(0, (avail - offset).max(0));
                    let r = if horizontal {
                        Rect::new(rect.x + offset + style.gap * k as i32, rect.y, len, rect.h)
                    } else {
                        Rect::new(rect.x, rect.y + offset + style.gap * k as i32, rect.w, len)
                    };
                    offset += len;
                    self.arrange_node(c, r, style, out);
                }
            }
            Layout::Stacking | Layout::Tabbed => {
                let th = style.title_height.max(0);
                let rows = if layout == Layout::Stacking { children.len() as i32 } else { 1 };
                let bar_h = (th * rows).min(rect.h);
                let mut tabs = Vec::with_capacity(children.len());
                for (k, &c) in children.iter().enumerate() {
                    let r = if layout == Layout::Stacking {
                        Rect::new(rect.x, rect.y + th * k as i32, rect.w, th)
                    } else {
                        let n = children.len() as i32;
                        let x0 = rect.w * k as i32 / n;
                        let x1 = rect.w * (k as i32 + 1) / n;
                        Rect::new(rect.x + x0, rect.y, x1 - x0, th)
                    };
                    let rep = self.focus_leaf_under(c);
                    let mut under = Vec::new();
                    self.leaves_under(c, &mut under);
                    tabs.push(Tab {
                        window: self.leaf_window(rep).expect("leaf"),
                        count: under.len(),
                        rect: r,
                        active: k == active,
                    });
                }
                out.title_bars.push(TitleBar { layout, tabs });
                let content = Rect::new(rect.x, rect.y + bar_h, rect.w, rect.h - bar_h);
                for (k, &c) in children.iter().enumerate() {
                    if k == active {
                        self.arrange_node(c, content, style, out);
                    } else {
                        self.n_mut(c).rect = content;
                        self.leaves_under(c, &mut out.hidden);
                    }
                }
            }
        }
    }

    /// Text description of the structure, e.g. `H[1 V[2 3]]`, for tests and
    /// debugging.
    pub fn describe(&self) -> String
    where
        W: std::fmt::Display,
    {
        fn go<W: Copy + Eq + Debug + std::fmt::Display>(t: &Tree<W>, id: NodeId, s: &mut String) {
            match &t.n(id).kind {
                Kind::Leaf(w) => s.push_str(&w.to_string()),
                Kind::Container { layout, children, .. } => {
                    s.push_str(match layout {
                        Layout::SplitH => "H[",
                        Layout::SplitV => "V[",
                        Layout::Stacking => "S[",
                        Layout::Tabbed => "T[",
                    });
                    for (k, &c) in children.iter().enumerate() {
                        if k > 0 {
                            s.push(' ');
                        }
                        go(t, c, s);
                    }
                    s.push(']');
                }
            }
        }
        let mut s = String::new();
        if let Some(r) = self.root {
            go(self, r, &mut s);
        }
        s
    }

    /// Check internal consistency (parent links, no stray nodes).
    #[doc(hidden)]
    pub fn check(&self) {
        let mut seen = 0;
        if let Some(r) = self.root {
            assert!(self.n(r).parent.is_none());
            assert!(!self.is_leaf(r));
            let mut stack = vec![r];
            while let Some(id) = stack.pop() {
                seen += 1;
                for &c in self.children(id) {
                    assert_eq!(self.n(c).parent, Some(id), "bad parent link");
                    stack.push(c);
                }
                if id != r && !self.is_leaf(id) {
                    assert!(!self.children(id).is_empty(), "empty non-root container");
                }
            }
        }
        assert_eq!(seen, self.nodes.iter().flatten().count(), "stray nodes");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(ws: &[u32]) -> Tree<u32> {
        let mut t = Tree::new();
        for &w in ws {
            t.insert(w);
            t.check();
        }
        t
    }

    #[test]
    fn insertion_rule_matches_agreed_diagram() {
        let mut t = Tree::new();
        t.insert(1);
        assert_eq!(t.describe(), "H[1]");
        t.insert(2);
        assert_eq!(t.describe(), "H[1 2]");
        t.insert(3); // focus on 2
        assert_eq!(t.describe(), "H[1 V[2 3]]");
        t.set_focus(1);
        t.insert(4);
        assert_eq!(t.describe(), "H[V[1 4] V[2 3]]");
        t.set_focus(3);
        t.insert(5); // already vertical: join
        assert_eq!(t.describe(), "H[V[1 4] V[2 3 5]]");
        assert_eq!(t.focused(), Some(5));
        t.check();
    }

    #[test]
    fn removal_flattens_and_resets_root() {
        let mut t = tree(&[1, 2, 3]);
        t.remove(3);
        t.check();
        assert_eq!(t.describe(), "H[1 2]");
        t.remove(1);
        assert_eq!(t.describe(), "H[2]");
        t.insert(4);
        assert_eq!(t.describe(), "H[2 4]");
        t.remove(2);
        t.remove(4);
        assert!(t.is_empty());
        t.check();
    }

    #[test]
    fn removing_left_column_hoists_right_column() {
        let mut t = tree(&[1, 2, 3]); // H[1 V[2 3]]
        t.remove(1);
        t.check();
        assert_eq!(t.describe(), "V[2 3]");
        t.insert(4); // focus 3, parent vertical
        assert_eq!(t.describe(), "V[2 3 4]");
    }

    #[test]
    fn focus_directions() {
        let mut t = tree(&[1, 2, 3]); // H[1 V[2 3]], focus 3
        assert_eq!(t.focus_dir(Dir::Up), Some(2));
        assert_eq!(t.focus_dir(Dir::Up), None);
        assert_eq!(t.focus_dir(Dir::Left), Some(1));
        // Coming back right lands on the last focused window of the column.
        assert_eq!(t.focus_dir(Dir::Right), Some(2));
        assert_eq!(t.focus_dir(Dir::Right), None);
    }

    #[test]
    fn move_swaps_enters_and_leaves() {
        let mut t = tree(&[1, 2, 3]); // H[1 V[2 3]]
        assert!(t.move_dir(3, Dir::Up));
        assert_eq!(t.describe(), "H[1 V[3 2]]");
        assert!(t.move_dir(3, Dir::Left)); // leaves the column
        t.check();
        assert_eq!(t.describe(), "H[1 3 2]");
        assert!(t.move_dir(1, Dir::Right));
        assert_eq!(t.describe(), "H[3 1 2]");
        assert!(!t.move_dir(3, Dir::Left)); // already at the edge of the root split
        assert!(t.move_dir(3, Dir::Up)); // new vertical root
        t.check();
        assert_eq!(t.describe(), "V[3 H[1 2]]");
        assert_eq!(t.focused(), Some(3));
    }

    #[test]
    fn move_into_neighbouring_column() {
        let mut t = tree(&[1, 2, 3]); // H[1 V[2 3]], focus 3
        t.set_focus(2);
        assert!(t.move_dir(1, Dir::Right)); // enters column after its focused child (2)
        t.check();
        assert_eq!(t.describe(), "V[2 1 3]");
    }

    #[test]
    fn stacking_arrangement() {
        let mut t = tree(&[1, 2, 3]); // H[1 V[2 3]]
        t.set_layout(3, Layout::Stacking);
        t.set_focus(2);
        let a = t.arrange(Rect::new(0, 0, 1000, 800), Style { gap: 0, title_height: 20 });
        assert_eq!(a.windows, vec![(1, Rect::new(0, 0, 500, 800)), (2, Rect::new(500, 40, 500, 760))]);
        assert_eq!(a.hidden, vec![3]);
        assert_eq!(a.title_bars.len(), 1);
        let tabs = &a.title_bars[0].tabs;
        assert_eq!(tabs.iter().map(|t| (t.window, t.active)).collect::<Vec<_>>(), vec![(2, true), (3, false)]);
        assert_eq!(tabs[1].rect, Rect::new(500, 20, 500, 20));
        // Up/down move through the stack.
        assert_eq!(t.focus_dir(Dir::Down), Some(3));
        assert!(t.toggle_split(3));
        assert_eq!(t.parent_layout(3), Some(Layout::SplitV));
    }

    #[test]
    fn new_window_joins_stack() {
        let mut t = tree(&[1, 2]);
        t.set_layout(2, Layout::Stacking);
        t.insert(3);
        assert_eq!(t.describe(), "S[1 2 3]");
    }

    #[test]
    fn explicit_split_on_lone_window() {
        let mut t = tree(&[1]);
        t.split(1, Axis::V);
        t.insert(2);
        assert_eq!(t.describe(), "V[1 2]");
        let mut t = tree(&[1, 2]); // H[1 2]
        t.split(2, Axis::H); // wrap 2 so the next one goes beside it
        t.insert(3);
        assert_eq!(t.describe(), "H[1 H[2 3]]");
        t.check();
    }

    #[test]
    fn gaps_and_weights() {
        let mut t = tree(&[1, 2]);
        let s = Style { gap: 10, title_height: 0 };
        let a = t.arrange(Rect::new(0, 0, 1010, 500), s);
        assert_eq!(a.windows, vec![(1, Rect::new(0, 0, 500, 500)), (2, Rect::new(510, 0, 500, 500))]);
        assert!(t.resize(1, Axis::H, 100));
        let a = t.arrange(Rect::new(0, 0, 1010, 500), s);
        assert_eq!(a.windows[0].1.w, 600);
        assert_eq!(a.windows[1].1, Rect::new(610, 0, 400, 500));
        assert!(!t.resize(1, Axis::V, 50)); // no vertical container
    }

    #[test]
    fn resize_edge_trades_with_one_neighbour() {
        let s = Style { gap: 0, title_height: 0 };
        let mut c = Tree::new();
        c.insert(1);
        c.insert(2);
        c.insert(3); // H[1 V[2 3]]
        assert!(c.move_dir(3, Dir::Right)); // three columns
        assert_eq!(c.describe(), "H[1 2 3]");
        c.arrange(Rect::new(0, 0, 900, 600), s);
        // Drag the right edge of 1 by +60: only 1 and 2 change.
        assert!(c.resize_edge(1, Dir::Right, 60));
        let a = c.arrange(Rect::new(0, 0, 900, 600), s);
        let w: Vec<i32> = a.windows.iter().map(|(_, r)| r.w).collect();
        assert_eq!(w, vec![360, 240, 300]);
        // Left edge of 3 outward by 30: 3 grows, 2 shrinks.
        assert!(c.resize_edge(3, Dir::Left, 30));
        let a = c.arrange(Rect::new(0, 0, 900, 600), s);
        let w: Vec<i32> = a.windows.iter().map(|(_, r)| r.w).collect();
        assert_eq!(w, vec![360, 210, 330]);
        // No neighbour beyond the screen edge.
        assert!(!c.resize_edge(3, Dir::Right, 10));
        assert!(!c.resize_edge(1, Dir::Up, 10));
        // Clamped: a neighbour never drops below 5%.
        assert!(c.resize_edge(1, Dir::Right, 10_000));
        let a = c.arrange(Rect::new(0, 0, 900, 600), s);
        assert_eq!(a.windows[1].1.w, 45);
        c.check();
    }

    #[test]
    fn sticky_slot_is_bottom_of_right_column() {
        let mut t: Tree<u32> = Tree::new();
        t.insert_sticky(9);
        assert_eq!(t.describe(), "H[9]");
        let mut t = tree(&[1]);
        t.insert_sticky(9);
        assert_eq!(t.describe(), "H[1 9]");
        assert_eq!(t.focused(), Some(1));
        let mut t = tree(&[1, 2]);
        t.insert_sticky(9);
        assert_eq!(t.describe(), "H[1 V[2 9]]");
        let mut t = tree(&[1, 2, 3]);
        t.set_focus(1);
        t.insert_sticky(9);
        assert_eq!(t.describe(), "H[1 V[2 3 9]]");
        assert_eq!(t.focused(), Some(1));
        t.remove(9);
        assert_eq!(t.describe(), "H[1 V[2 3]]");
        t.check();
    }

    #[test]
    fn random_operations_keep_tree_consistent() {
        // Small deterministic PRNG; exercises insert/remove/move/layout mixes.
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let dirs = [Dir::Left, Dir::Right, Dir::Up, Dir::Down];
        let layouts = [Layout::SplitH, Layout::SplitV, Layout::Stacking, Layout::Tabbed];
        let mut t: Tree<u32> = Tree::new();
        let mut live: Vec<u32> = Vec::new();
        let mut next = 0;
        for _ in 0..5000 {
            match rnd(9) {
                0 | 1 => {
                    next += 1;
                    if rnd(5) == 0 { t.insert_sticky(next) } else { t.insert(next) }
                    live.push(next);
                }
                2 if !live.is_empty() => {
                    let w = live.remove(rnd(live.len() as u64) as usize);
                    assert!(t.remove(w));
                }
                3 if !live.is_empty() => {
                    let w = live[rnd(live.len() as u64) as usize];
                    t.move_dir(w, dirs[rnd(4) as usize]);
                }
                4 => {
                    t.focus_dir(dirs[rnd(4) as usize]);
                }
                5 if !live.is_empty() => {
                    let w = live[rnd(live.len() as u64) as usize];
                    t.set_layout(w, layouts[rnd(4) as usize]);
                }
                6 if !live.is_empty() => {
                    let w = live[rnd(live.len() as u64) as usize];
                    if rnd(2) == 0 { t.split(w, Axis::V) } else { t.toggle_split(w) };
                }
                7 if !live.is_empty() => {
                    let w = live[rnd(live.len() as u64) as usize];
                    t.set_focus(w);
                }
                _ => {
                    let a = t.arrange(Rect::new(0, 0, 1920, 1080), Style { gap: 3, title_height: 20 });
                    assert_eq!(a.windows.len() + a.hidden.len(), live.len());
                    if !live.is_empty() {
                        let w = live[rnd(live.len() as u64) as usize];
                        t.resize(w, if rnd(2) == 0 { Axis::H } else { Axis::V }, 40);
                    }
                }
            }
            t.check();
            let mut have = t.windows();
            have.sort();
            let mut want = live.clone();
            want.sort();
            assert_eq!(have, want);
            if !live.is_empty() {
                assert!(t.focused().is_some());
            }
        }
    }
}
