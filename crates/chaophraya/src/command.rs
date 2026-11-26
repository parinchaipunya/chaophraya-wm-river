//! Sway-flavoured command language shared by key bindings and IPC.

use chaophraya_tree::{Axis, Dir, Layout};

pub const MAX_WORKSPACE: u8 = 10;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Run a shell command.
    Spawn(String),
    Close,
    Exit,
    Reload,
    Focus(Dir),
    /// Switch focus between tiled and floating windows.
    FocusModeToggle,
    Move(Dir),
    Layout(Layout),
    LayoutToggleSplit,
    Split(Axis),
    FullscreenToggle,
    FloatingToggle,
    StickyToggle,
    Workspace(u8),
    MoveToWorkspace(u8),
    /// Open workspace N on output NAME from now on (sway `workspace N output NAME`).
    PinWorkspace(u8, String),
    /// Move the focused workspace to output NAME.
    MoveWorkspaceToOutput(String),
    /// Grow (positive) or shrink (negative) the focused window by px.
    Resize { axis: Axis, px: i32 },
    ScratchpadMove,
    ScratchpadShow,
    /// Focus a window by its id (from `chaophraya msg get_windows`).
    FocusId(u32),
    /// Switch keyboard layout: an index into the configured layouts, or the next one.
    KeyboardLayout(Option<u32>),
    /// Mark hidden windows of an app as urgent (river can't report urgency,
    /// so a notification script calls this).
    UrgentAppId(String),
    Nop,
}

fn dir(s: &str) -> Option<Dir> {
    Some(match s {
        "left" => Dir::Left,
        "right" => Dir::Right,
        "up" => Dir::Up,
        "down" => Dir::Down,
        _ => return None,
    })
}

fn workspace(words: &[&str]) -> Result<u8, String> {
    // Accept "3" and "number 3".
    let n = match words {
        ["number", n] | [n] => n,
        _ => return Err("expected a workspace number".into()),
    };
    match n.parse::<u8>() {
        Ok(n) if (1..=MAX_WORKSPACE).contains(&n) => Ok(n),
        _ => Err(format!("workspace must be 1-{MAX_WORKSPACE}, got '{n}'")),
    }
}

impl Command {
    pub fn parse(input: &str) -> Result<Command, String> {
        let input = input.trim();
        let (head, rest) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
        let rest = rest.trim();
        let words: Vec<&str> = rest.split_whitespace().collect();
        let bad = || format!("invalid command '{input}'");

        Ok(match head {
            "spawn" | "exec" => {
                let cmd = rest.strip_prefix("--no-startup-id").unwrap_or(rest).trim();
                if cmd.is_empty() {
                    return Err("spawn needs a command".into());
                }
                Command::Spawn(cmd.to_owned())
            }
            "close" | "kill" => Command::Close,
            "exit" => Command::Exit,
            "reload" => Command::Reload,
            "nop" => Command::Nop,
            "focus" => match words.as_slice() {
                ["mode_toggle"] => Command::FocusModeToggle,
                ["id", n] => Command::FocusId(n.parse().map_err(|_| bad())?),
                [d] => Command::Focus(dir(d).ok_or_else(bad)?),
                _ => return Err(bad()),
            },
            "move" => match words.as_slice() {
                ["scratchpad"] => Command::ScratchpadMove,
                ["workspace", "to", "output", name] | ["workspace", "output", name] => {
                    Command::MoveWorkspaceToOutput((*name).to_owned())
                }
                [d] if dir(d).is_some() => Command::Move(dir(d).unwrap()),
                // move [container|window] [to] workspace [number] N
                _ => {
                    let w: Vec<&str> = words
                        .iter()
                        .copied()
                        .skip_while(|w| matches!(*w, "container" | "window" | "to"))
                        .collect();
                    match w.split_first() {
                        Some((&"workspace", n)) => Command::MoveToWorkspace(workspace(n)?),
                        Some((&"scratchpad", [])) => Command::ScratchpadMove,
                        _ => return Err(bad()),
                    }
                }
            },
            "workspace" => match words.iter().position(|w| *w == "output") {
                Some(i) => match &words[i + 1..] {
                    [name] => Command::PinWorkspace(workspace(&words[..i])?, (*name).to_owned()),
                    _ => return Err(bad()),
                },
                None => Command::Workspace(workspace(&words)?),
            },
            "layout" => match words.as_slice() {
                ["splith"] => Command::Layout(Layout::SplitH),
                ["splitv"] => Command::Layout(Layout::SplitV),
                ["stacking"] => Command::Layout(Layout::Stacking),
                ["tabbed"] => Command::Layout(Layout::Tabbed),
                ["toggle", "split"] | ["toggle"] => Command::LayoutToggleSplit,
                _ => return Err(bad()),
            },
            "splith" => Command::Split(Axis::H),
            "splitv" => Command::Split(Axis::V),
            "split" => match words.as_slice() {
                ["h" | "horizontal"] => Command::Split(Axis::H),
                ["v" | "vertical"] => Command::Split(Axis::V),
                _ => return Err(bad()),
            },
            "fullscreen" => Command::FullscreenToggle,
            "urgent" => match rest.strip_prefix("app_id") {
                Some(name) if !name.trim().is_empty() => Command::UrgentAppId(name.trim().to_owned()),
                _ => return Err("usage: urgent app_id <name>".into()),
            },
            "keyboard" => match words.as_slice() {
                ["layout", "next"] => Command::KeyboardLayout(None),
                ["layout", n] => Command::KeyboardLayout(Some(n.parse().map_err(|_| bad())?)),
                _ => return Err(bad()),
            },
            "floating" => match words.as_slice() {
                ["toggle"] | [] => Command::FloatingToggle,
                _ => return Err(bad()),
            },
            "sticky" => match words.as_slice() {
                ["toggle"] | [] => Command::StickyToggle,
                _ => return Err(bad()),
            },
            "scratchpad" => match words.as_slice() {
                ["show"] => Command::ScratchpadShow,
                _ => return Err(bad()),
            },
            // resize grow|shrink width|height N [px]
            "resize" => match words.as_slice() {
                [gs @ ("grow" | "shrink"), wh @ ("width" | "height"), n, rest @ ..]
                    if rest.is_empty() || rest == ["px"] =>
                {
                    let px: i32 = n.parse().map_err(|_| bad())?;
                    let px = if *gs == "grow" { px } else { -px };
                    let axis = if *wh == "width" { Axis::H } else { Axis::V };
                    Command::Resize { axis, px }
                }
                _ => return Err(bad()),
            },
            _ => return Err(format!("unknown command '{head}'")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sway_style_commands() {
        let p = |s| Command::parse(s).unwrap();
        assert_eq!(p("exec --no-startup-id foot -e tmux"), Command::Spawn("foot -e tmux".into()));
        assert_eq!(p("kill"), Command::Close);
        assert_eq!(p("focus left"), Command::Focus(Dir::Left));
        assert_eq!(p("focus mode_toggle"), Command::FocusModeToggle);
        assert_eq!(p("move down"), Command::Move(Dir::Down));
        assert_eq!(p("move container to workspace number 3"), Command::MoveToWorkspace(3));
        assert_eq!(p("move to workspace 10"), Command::MoveToWorkspace(10));
        assert_eq!(p("workspace number 1"), Command::Workspace(1));
        assert_eq!(p("layout toggle split"), Command::LayoutToggleSplit);
        assert_eq!(p("layout stacking"), Command::Layout(Layout::Stacking));
        assert_eq!(p("splitv"), Command::Split(Axis::V));
        assert_eq!(p("resize shrink width 10 px"), Command::Resize { axis: Axis::H, px: -10 });
        assert_eq!(p("resize grow height 10"), Command::Resize { axis: Axis::V, px: 10 });
        assert_eq!(p("move scratchpad"), Command::ScratchpadMove);
        assert_eq!(p("scratchpad show"), Command::ScratchpadShow);
        assert_eq!(p("focus id 7"), Command::FocusId(7));
        assert_eq!(p("keyboard layout 0"), Command::KeyboardLayout(Some(0)));
        assert_eq!(p("keyboard layout next"), Command::KeyboardLayout(None));
        assert_eq!(p("urgent app_id Thunderbird"), Command::UrgentAppId("Thunderbird".into()));
        assert_eq!(p("workspace 10 output HDMI-A-1"), Command::PinWorkspace(10, "HDMI-A-1".into()));
        assert_eq!(p("workspace number 3 output eDP-1"), Command::PinWorkspace(3, "eDP-1".into()));
        assert_eq!(p("move workspace to output DP-1"), Command::MoveWorkspaceToOutput("DP-1".into()));
        assert!(Command::parse("workspace 2 output").is_err());
        assert!(Command::parse("urgent app_id").is_err());
        for bad in ["workspace 11", "workspace 0", "focus sideways", "frobnicate", "spawn", "resize grow width x"] {
            assert!(Command::parse(bad).is_err(), "{bad} should fail");
        }
    }
}
