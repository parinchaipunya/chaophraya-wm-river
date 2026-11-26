//! Unix-socket IPC: `chaophraya msg <command>`, `chaophraya subscribe`, and
//! `chaophraya waybar ...` helpers for waybar custom modules.
//!
//! Protocol: one line per request. A command line gets one reply line ("ok" or
//! "error: ..."). The line "subscribe" turns the connection into a stream of
//! JSON state lines, one per change. "get_state" replies with one JSON line.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::command::{Command, MAX_WORKSPACE};
use crate::wm::{Pending, State};

/// Socket a client should talk to: `$CHAOPHRAYA_SOCK` (set by the WM for
/// everything it starts), else the one for the current `$WAYLAND_DISPLAY`.
pub fn socket_path() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("CHAOPHRAYA_SOCK") {
        return Ok(PathBuf::from(p));
    }
    own_socket_path()
}

/// Socket the WM itself listens on. Always derived from its own Wayland
/// display, never from an inherited `$CHAOPHRAYA_SOCK`: a chaophraya started
/// from inside another chaophraya session must not take over the parent's
/// socket.
pub fn own_socket_path() -> Result<PathBuf, String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    let display = std::env::var("WAYLAND_DISPLAY").map_err(|_| "WAYLAND_DISPLAY is not set")?;
    Ok(PathBuf::from(runtime).join(format!("chaophraya.{display}.sock")))
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct WorkspaceInfo {
    pub num: u8,
    pub output: Option<String>,
    pub visible: bool,
    pub focused: bool,
    pub windows: usize,
    /// Has a window that wants attention.
    #[serde(default)]
    pub urgent: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub focused_output: Option<String>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub title: Option<String>,
    pub app_id: Option<String>,
    /// Layout of the focused window's container, or "floating".
    pub layout: Option<String>,
    pub sticky: bool,
    /// Active keyboard layout code, e.g. "US".
    pub keyboard_layout: Option<String>,
    /// A window's inhibit_idle rule is currently keeping the screen awake.
    #[serde(default)]
    pub idle_inhibited: bool,
}

impl Snapshot {
    pub fn of(state: &State) -> Snapshot {
        let cur = state.current_ws();
        let workspaces = (1..=MAX_WORKSPACE)
            .filter_map(|n| {
                let windows = state.windows.values().filter(|w| w.ws == n).count();
                let out = state.outputs.iter().find(|o| o.ws == n);
                (windows > 0 || out.is_some()).then(|| WorkspaceInfo {
                    num: n,
                    output: out.map(|o| state.output_name(o)),
                    visible: out.is_some(),
                    focused: n == cur,
                    windows,
                    urgent: state.windows.values().any(|w| w.ws == n && w.urgent),
                })
            })
            .collect();
        let fw = state.focused.and_then(|id| state.windows.get(&id).map(|w| (id, w)));
        let layout = fw.map(|(id, w)| {
            if w.floating {
                "floating".to_owned()
            } else {
                match state.trees[w.ws as usize].parent_layout(id) {
                    Some(chaophraya_tree::Layout::SplitH) => "splith",
                    Some(chaophraya_tree::Layout::SplitV) => "splitv",
                    Some(chaophraya_tree::Layout::Stacking) => "stacking",
                    Some(chaophraya_tree::Layout::Tabbed) => "tabbed",
                    None => "splith",
                }
                .to_owned()
            }
        });
        let focused_output = state
            .outputs
            .iter()
            .find(|o| o.ws == cur)
            .map(|o| state.output_name(o));
        Snapshot {
            focused_output,
            workspaces,
            title: fw.map(|(_, w)| w.title.clone()),
            app_id: fw.map(|(_, w)| w.app_id.clone()),
            layout,
            sticky: fw.is_some_and(|(_, w)| w.sticky),
            keyboard_layout: state.layout_code(),
            idle_inhibited: state.idle_inhibited(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app_id: String,
    pub workspace: Option<u8>,
    pub floating: bool,
    pub sticky: bool,
    pub fullscreen: bool,
    pub focused: bool,
    pub visible: bool,
    pub urgent: bool,
    /// Content rectangle in global coordinates [x, y, width, height].
    pub rect: [i32; 4],
}

pub fn windows_of(state: &State) -> Vec<WindowInfo> {
    state
        .windows
        .iter()
        .map(|(id, w)| {
            let (x, y) = w.target.unwrap_or((0, 0));
            WindowInfo {
                id: *id,
                title: w.title.clone(),
                app_id: w.app_id.clone(),
                workspace: (w.ws != 0).then_some(w.ws),
                floating: w.floating,
                sticky: w.sticky,
                fullscreen: w.fullscreen,
                focused: state.focused == Some(*id),
                visible: w.shown == Some(true),
                urgent: w.urgent,
                rect: [x, y, w.dims.0, w.dims.1],
            }
        })
        .collect()
}

struct Client {
    id: u64,
    stream: UnixStream,
    buf: Vec<u8>,
    subscribed: bool,
    dead: bool,
}

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    clients: Vec<Client>,
    next_id: u64,
    last: Option<String>,
}

impl Server {
    pub fn bind() -> Result<Server, String> {
        let path = own_socket_path()?;
        if UnixStream::connect(&path).is_ok() {
            return Err(format!("{} is in use (is chaophraya already running?)", path.display()));
        }
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        // Children (scripts, waybar) find the socket through this.
        std::env::set_var("CHAOPHRAYA_SOCK", &path);
        Ok(Server { listener, path, clients: Vec::new(), next_id: 1, last: None })
    }

    pub fn fds(&self) -> Vec<RawFd> {
        let mut v = vec![self.listener.as_raw_fd()];
        v.extend(self.clients.iter().map(|c| c.stream.as_raw_fd()));
        v
    }

    /// Accept connections and read requests. Commands go to `state.queue`;
    /// returns true if a manage sequence is needed.
    pub fn poll(&mut self, state: &mut State) -> bool {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(true);
                    self.clients.push(Client {
                        id: self.next_id,
                        stream,
                        buf: Vec::new(),
                        subscribed: false,
                        dead: false,
                    });
                    self.next_id += 1;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("chaophraya: ipc accept: {e}");
                    break;
                }
            }
        }
        let mut need_manage = false;
        for c in &mut self.clients {
            let mut chunk = [0u8; 4096];
            loop {
                match c.stream.read(&mut chunk) {
                    Ok(0) => {
                        c.dead = true;
                        break;
                    }
                    Ok(n) => c.buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        c.dead = true;
                        break;
                    }
                }
            }
            while let Some(pos) = c.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = c.buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line).trim().to_owned();
                if line.is_empty() {
                    continue;
                }
                match line.as_str() {
                    "subscribe" => {
                        c.subscribed = true;
                        let snap = serde_json::to_string(&Snapshot::of(state)).unwrap();
                        send(c, &snap);
                    }
                    "get_windows" => {
                        let list = serde_json::to_string(&windows_of(state)).unwrap();
                        send(c, &list);
                    }
                    "get_state" => {
                        let snap = serde_json::to_string(&Snapshot::of(state)).unwrap();
                        send(c, &snap);
                    }
                    _ => match Command::parse(&line) {
                        Ok(command) => {
                            state.queue.push(Pending { command, client: Some(c.id) });
                            need_manage = true;
                        }
                        Err(e) => send(c, &format!("error: {e}")),
                    },
                }
            }
        }
        self.clients.retain(|c| !c.dead);
        need_manage
    }

    /// Deliver command replies and state changes after a manage sequence.
    pub fn flush(&mut self, state: &mut State) {
        for (id, reply) in std::mem::take(&mut state.replies) {
            if let Some(c) = self.clients.iter_mut().find(|c| c.id == id) {
                send(c, &reply);
            }
        }
        if std::mem::take(&mut state.state_changed) {
            let snap = serde_json::to_string(&Snapshot::of(state)).unwrap();
            if self.last.as_deref() != Some(snap.as_str()) {
                for c in self.clients.iter_mut().filter(|c| c.subscribed) {
                    send(c, &snap);
                }
                self.last = Some(snap);
            }
        }
        self.clients.retain(|c| !c.dead);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn send(c: &mut Client, line: &str) {
    // Small writes on a local socket; a client that can't keep up is dropped.
    let mut data = line.as_bytes().to_vec();
    data.push(b'\n');
    let _ = c.stream.set_nonblocking(false);
    let _ = c.stream.set_write_timeout(Some(std::time::Duration::from_millis(200)));
    if c.stream.write_all(&data).is_err() {
        c.dead = true;
    }
    let _ = c.stream.set_nonblocking(true);
}

// ---- client side ----------------------------------------------------------

fn connect() -> Result<UnixStream, String> {
    let path = socket_path()?;
    UnixStream::connect(&path).map_err(|e| format!("cannot connect to {}: {e}", path.display()))
}

/// `chaophraya msg <command...>`: send one command, print the reply.
pub fn msg(words: &[String]) -> Result<(), String> {
    let line = words.join(" ");
    let mut s = connect()?;
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply).map_err(|e| e.to_string())?;
    let reply = reply.trim();
    if let Some(e) = reply.strip_prefix("error: ") {
        return Err(e.to_owned());
    }
    if reply != "ok" {
        println!("{reply}");
    }
    Ok(())
}

/// Stream snapshots to `f` until the connection closes.
fn each_snapshot(mut f: impl FnMut(&Snapshot)) -> Result<(), String> {
    let mut s = connect()?;
    writeln!(s, "subscribe").map_err(|e| e.to_string())?;
    for line in BufReader::new(s).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let snap: Snapshot = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        f(&snap);
    }
    Ok(())
}

pub fn subscribe() -> Result<(), String> {
    let mut s = connect()?;
    writeln!(s, "subscribe").map_err(|e| e.to_string())?;
    let mut out = std::io::stdout().lock();
    for line in BufReader::new(s).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
    Ok(())
}

#[derive(Serialize, PartialEq, Clone)]
struct WaybarLine {
    text: String,
    tooltip: String,
    class: String,
}

/// `chaophraya waybar workspace N | title | layout`: JSON lines for a waybar
/// custom module with `"return-type": "json"`.
pub fn waybar(args: &[String]) -> Result<(), String> {
    let what = args.first().map(String::as_str).unwrap_or("");
    let ws: Option<u8> = match what {
        "workspace" => Some(
            args.get(1)
                .and_then(|n| n.parse().ok())
                .filter(|n| (1..=MAX_WORKSPACE).contains(n))
                .ok_or("usage: chaophraya waybar workspace <1-10>")?,
        ),
        "title" | "layout" | "keyboard" => None,
        _ => return Err("usage: chaophraya waybar workspace <N> | title | layout | keyboard".into()),
    };
    let mut last: Option<WaybarLine> = None;
    let mut out = std::io::stdout().lock();
    each_snapshot(|snap| {
        let line = format_waybar(what, ws, snap);
        if last.as_ref() != Some(&line) {
            let _ = writeln!(out, "{}", serde_json::to_string(&line).unwrap());
            let _ = out.flush();
            last = Some(line);
        }
    })
}

fn format_waybar(what: &str, ws: Option<u8>, snap: &Snapshot) -> WaybarLine {
    match (what, ws) {
        ("workspace", Some(n)) => match snap.workspaces.iter().find(|w| w.num == n) {
            // Empty text hides the module, like sway hides empty workspaces.
            None => WaybarLine { text: String::new(), tooltip: String::new(), class: "empty".into() },
            Some(w) => {
                let class = if w.focused {
                    "focused"
                } else if w.urgent {
                    "urgent"
                } else if w.visible {
                    "visible"
                } else {
                    "occupied"
                };
                WaybarLine {
                    text: n.to_string(),
                    tooltip: format!("{} window(s){}", w.windows, w.output.as_deref().map(|o| format!(" on {o}")).unwrap_or_default()),
                    class: class.into(),
                }
            }
        },
        ("keyboard", _) => {
            let code = snap.keyboard_layout.clone().unwrap_or_default();
            WaybarLine { tooltip: code.clone(), class: code.to_lowercase(), text: code }
        }
        ("title", _) => {
            let title = snap.title.clone().unwrap_or_default();
            let app = snap.app_id.clone().unwrap_or_default();
            WaybarLine { tooltip: app, class: if snap.sticky { "sticky".into() } else { String::new() }, text: title }
        }
        _ => {
            let text = match snap.layout.as_deref() {
                Some("splith") => "[H]",
                Some("splitv") => "[V]",
                Some("stacking") => "[S]",
                Some("tabbed") => "[T]",
                Some("floating") => "[F]",
                _ => "",
            };
            WaybarLine { text: text.into(), tooltip: snap.layout.clone().unwrap_or_default(), class: snap.layout.clone().unwrap_or_default() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waybar_workspace_classes() {
        let snap = Snapshot {
            workspaces: vec![
                WorkspaceInfo { num: 1, output: Some("eDP-1".into()), visible: true, focused: true, windows: 2, urgent: false },
                WorkspaceInfo { num: 3, output: None, visible: false, focused: false, windows: 1, urgent: false },
                WorkspaceInfo { num: 4, output: None, visible: false, focused: false, windows: 1, urgent: true },
            ],
            ..Default::default()
        };
        assert_eq!(format_waybar("workspace", Some(4), &snap).class, "urgent");
        assert_eq!(format_waybar("workspace", Some(1), &snap).class, "focused");
        assert_eq!(format_waybar("workspace", Some(3), &snap).class, "occupied");
        assert_eq!(format_waybar("workspace", Some(2), &snap).text, "");
    }
}
