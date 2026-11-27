//! TOML configuration (`$XDG_CONFIG_HOME/chaophraya/config.toml`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use regex::Regex;
use serde::Deserialize;

use crate::command::{Command, MAX_WORKSPACE};
use crate::protocol::river_seat_v1::Modifiers;

/// Built-in config used when the user has none.
pub const DEFAULT_CONFIG: &str = include_str!("../default-config.toml");

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Raw {
    general: General,
    colors: RawColors,
    bindings: BTreeMap<String, String>,
    rules: Vec<RawRule>,
    autostart: Autostart,
    /// Workspace number -> output name it should open on.
    workspace_outputs: BTreeMap<String, String>,
    keyboard: Keyboard,
    input: Vec<InputRule>,
    /// Binding modes: name -> bindings active while that mode is on.
    modes: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Keyboard {
    pub model: String,
    /// Comma-separated xkb layouts, e.g. "us,th".
    pub layout: String,
    pub variant: String,
    pub options: String,
    /// Keys per second.
    pub repeat_rate: i32,
    /// Milliseconds before repeating starts.
    pub repeat_delay: i32,
}

impl Default for Keyboard {
    fn default() -> Self {
        Keyboard {
            model: String::new(),
            layout: "us".into(),
            variant: String::new(),
            options: String::new(),
            repeat_rate: 25,
            repeat_delay: 600,
        }
    }
}

/// Settings for matching libinput devices (sway `input` blocks).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct InputRule {
    /// "touchpad", "pointer", "keyboard", "*", or part of the device name.
    #[serde(rename = "match")]
    pub matches: String,
    pub tap: Option<bool>,
    pub natural_scroll: Option<bool>,
    pub dwt: Option<bool>,
    pub middle_emulation: Option<bool>,
    /// -1.0 .. 1.0
    pub accel_speed: Option<f64>,
    /// "flat" or "adaptive"
    pub accel_profile: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub border_width: i32,
    pub gaps_inner: i32,
    pub gaps_outer: i32,
    pub focus_follows_mouse: bool,
    /// Modifier for mouse window management: with the left button drag
    /// floating windows; with the right button resize floating windows or,
    /// on tiled windows, move the split edge nearest the cursor.
    pub floating_modifier: String,
    pub title_height: i32,
    pub font: String,
    /// Title text; {title} and {app_id} are replaced.
    pub title_format: String,
    /// Move the cursor when a command moves focus to another output:
    /// "output" (to its centre), "container" (to the focused window) or "none".
    pub mouse_warping: String,
    /// Park every output except these (by name, e.g. ["eDP-1"]) when it
    /// appears: it stays on but gets no workspace and is skipped by focus,
    /// moves and cursor warps, until `output NAME unpark`. Empty: no parking.
    pub park_outputs_except: Vec<String>,
}

impl Default for General {
    fn default() -> Self {
        General {
            border_width: 2,
            gaps_inner: 0,
            gaps_outer: 0,
            focus_follows_mouse: true,
            floating_modifier: "Super".into(),
            title_height: 20,
            font: "Noto Sans 10".into(),
            title_format: "{title}".into(),
            mouse_warping: "output".into(),
            park_outputs_except: Vec::new(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
struct RawColors {
    focused: String,
    unfocused: String,
    urgent: String,
    focused_title: String,
    focused_title_text: String,
    active_title: String,
    active_title_text: String,
    unfocused_title: String,
    unfocused_title_text: String,
    title_separator: String,
}

impl Default for RawColors {
    fn default() -> Self {
        RawColors {
            focused: "#ff6b00".into(),
            unfocused: "#505050".into(),
            urgent: "#d08770".into(),
            focused_title: "#ff6b00".into(),
            focused_title_text: "#ececec".into(),
            active_title: "#5f5f5f".into(),
            active_title_text: "#ececec".into(),
            unfocused_title: "#505050".into(),
            unfocused_title_text: "#a5a5a5".into(),
            title_separator: "#2e3440".into(),
        }
    }
}

/// 8-bit RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub fn parse(s: &str) -> Result<Color, String> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        let v = u32::from_str_radix(hex, 16).map_err(|_| format!("invalid color '{s}'"))?;
        match hex.len() {
            6 => Ok(Color([(v >> 16) as u8, (v >> 8) as u8, v as u8, 0xff])),
            8 => Ok(Color([(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8])),
            _ => Err(format!("invalid color '{s}' (use #rrggbb or #rrggbbaa)")),
        }
    }

    /// Pre-multiplied 32-bit channels as river's set_borders wants them.
    pub fn river(self) -> [u32; 4] {
        let [r, g, b, a] = self.0.map(u32::from);
        let pm = |c: u32| c * a / 255 * 0x0101_0101;
        [pm(r), pm(g), pm(b), a * 0x0101_0101]
    }
}

#[derive(Clone, Debug)]
pub struct Colors {
    pub focused: Color,
    pub unfocused: Color,
    pub urgent: Color,
    pub focused_title: Color,
    pub focused_title_text: Color,
    pub active_title: Color,
    pub active_title_text: Color,
    pub unfocused_title: Color,
    pub unfocused_title_text: Color,
    pub title_separator: Color,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawRule {
    app_id: Option<String>,
    title: Option<String>,
    floating: Option<bool>,
    sticky: Option<bool>,
    workspace: Option<u8>,
    /// Floating size in px.
    size: Option<[i32; 2]>,
    /// Floating size in percent of the output.
    size_percent: Option<[i32; 2]>,
    /// Close the window as soon as it appears.
    kill: bool,
    scratchpad: bool,
    /// Keep the screen from idling: "fullscreen", "visible", "focus" or "open".
    inhibit_idle: Option<String>,
    /// Border width for this window, overriding general.border_width.
    border_width: Option<i32>,
}

/// When a window keeps the session from going idle (sway's `inhibit_idle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleMode {
    Fullscreen,
    Visible,
    Focus,
    Open,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub app_id: Option<Regex>,
    pub title: Option<Regex>,
    pub floating: Option<bool>,
    pub sticky: Option<bool>,
    pub workspace: Option<u8>,
    pub size: Option<Size>,
    pub kill: bool,
    pub scratchpad: bool,
    pub inhibit_idle: Option<IdleMode>,
    pub border_width: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Px(i32, i32),
    Percent(i32, i32),
}

impl Rule {
    pub fn matches(&self, app_id: &str, title: &str) -> bool {
        self.app_id.as_ref().is_none_or(|r| r.is_match(app_id)) && self.title.as_ref().is_none_or(|r| r.is_match(title))
    }
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Autostart {
    /// Run once when chaophraya starts.
    pub exec: Vec<String>,
    /// Run on start and on every reload.
    pub exec_always: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub mods: Modifiers,
    pub keysym: u32,
    pub command: Command,
    /// Keep working while the session is locked (media keys).
    pub locked: bool,
    /// Binding mode it belongs to; None: the default mode.
    pub mode: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub general: General,
    pub colors: Colors,
    pub bindings: Vec<Binding>,
    pub floating_mod: Modifiers,
    pub rules: Vec<Rule>,
    pub autostart: Autostart,
    pub workspace_outputs: BTreeMap<u8, String>,
    pub keyboard: Keyboard,
    pub inputs: Vec<InputRule>,
    /// Names of the [modes.NAME] tables.
    pub modes: Vec<String>,
}

pub fn parse_mods(spec: &[&str]) -> Result<Modifiers, String> {
    let mut mods = Modifiers::empty();
    for m in spec {
        mods |= match m.to_ascii_lowercase().as_str() {
            "super" | "mod4" | "logo" | "win" => Modifiers::Mod4,
            "alt" | "mod1" => Modifiers::Mod1,
            "ctrl" | "control" => Modifiers::Ctrl,
            "shift" => Modifiers::Shift,
            "mod3" => Modifiers::Mod3,
            "mod5" | "altgr" => Modifiers::Mod5,
            "none" => Modifiers::empty(),
            other => return Err(format!("unknown modifier '{other}'")),
        };
    }
    Ok(mods)
}

fn parse_binding(key: &str, command: &str) -> Result<Binding, String> {
    let (locked, key) = match key.strip_prefix("locked:") {
        Some(k) => (true, k),
        None => (false, key),
    };
    let parts: Vec<&str> = key.split('+').map(str::trim).collect();
    let (name, mods) = parts.split_last().ok_or("empty key")?;
    let mods = parse_mods(mods)?;
    let sym = xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_NO_FLAGS);
    let sym = if sym.raw() == 0 {
        xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_CASE_INSENSITIVE)
    } else {
        sym
    };
    if sym.raw() == 0 {
        return Err(format!("unknown key '{name}'"));
    }
    Ok(Binding { mods, keysym: sym.raw(), command: Command::parse(command)?, locked, mode: None })
}

impl Config {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("chaophraya").join("config.toml")
    }

    /// Load the user's config, or the built-in default if there is none.
    pub fn load() -> Result<Config, String> {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::parse(DEFAULT_CONFIG),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string())?;
        let c = &raw.colors;
        let colors = Colors {
            focused: Color::parse(&c.focused)?,
            unfocused: Color::parse(&c.unfocused)?,
            urgent: Color::parse(&c.urgent)?,
            focused_title: Color::parse(&c.focused_title)?,
            focused_title_text: Color::parse(&c.focused_title_text)?,
            active_title: Color::parse(&c.active_title)?,
            active_title_text: Color::parse(&c.active_title_text)?,
            unfocused_title: Color::parse(&c.unfocused_title)?,
            unfocused_title_text: Color::parse(&c.unfocused_title_text)?,
            title_separator: Color::parse(&c.title_separator)?,
        };
        let mut bindings = Vec::new();
        for (key, cmd) in &raw.bindings {
            bindings.push(parse_binding(key, cmd).map_err(|e| format!("binding '{key}': {e}"))?);
        }
        let modes: Vec<String> = raw.modes.keys().cloned().collect();
        if modes.iter().any(|m| m == "default") {
            return Err("modes: \"default\" is the normal mode; use [bindings] for it".into());
        }
        for (mode, keys) in &raw.modes {
            for (key, cmd) in keys {
                let mut b = parse_binding(key, cmd).map_err(|e| format!("modes.{mode} binding '{key}': {e}"))?;
                b.mode = Some(mode.clone());
                bindings.push(b);
            }
        }
        for b in &bindings {
            if let Command::Mode(m) = &b.command {
                if m != "default" && !modes.contains(m) {
                    return Err(format!("binding to unknown mode '{m}' (add a [modes.{m}] table)"));
                }
            }
        }
        let mut rules = Vec::new();
        for (i, r) in raw.rules.into_iter().enumerate() {
            let re = |p: Option<String>| -> Result<Option<Regex>, String> {
                p.map(|p| Regex::new(&format!("^(?:{p})$")).map_err(|e| format!("rule {}: {e}", i + 1)))
                    .transpose()
            };
            if let Some(ws) = r.workspace {
                if !(1..=MAX_WORKSPACE).contains(&ws) {
                    return Err(format!("rule {}: workspace must be 1-{MAX_WORKSPACE}", i + 1));
                }
            }
            let size = match (r.size, r.size_percent) {
                (Some([w, h]), _) => Some(Size::Px(w, h)),
                (None, Some([w, h])) => Some(Size::Percent(w, h)),
                (None, None) => None,
            };
            let inhibit_idle = match r.inhibit_idle.as_deref() {
                None => None,
                Some("fullscreen") => Some(IdleMode::Fullscreen),
                Some("visible") => Some(IdleMode::Visible),
                Some("focus") => Some(IdleMode::Focus),
                Some("open") => Some(IdleMode::Open),
                Some(other) => {
                    return Err(format!(
                        "rule {}: inhibit_idle must be fullscreen, visible, focus or open, not '{other}'",
                        i + 1
                    ))
                }
            };
            rules.push(Rule {
                app_id: re(r.app_id)?,
                title: re(r.title)?,
                floating: r.floating,
                sticky: r.sticky,
                workspace: r.workspace,
                size,
                kill: r.kill,
                scratchpad: r.scratchpad,
                inhibit_idle,
                border_width: r.border_width,
            });
        }
        let mut workspace_outputs = BTreeMap::new();
        for (ws, out) in raw.workspace_outputs {
            let n: u8 = ws.parse().map_err(|_| format!("workspace_outputs: bad workspace '{ws}'"))?;
            workspace_outputs.insert(n, out);
        }
        if !matches!(raw.general.mouse_warping.as_str(), "output" | "container" | "none") {
            return Err(format!(
                "general.mouse_warping must be output, container or none, not '{}'",
                raw.general.mouse_warping
            ));
        }
        let floating_mod = parse_mods(&raw.general.floating_modifier.split('+').collect::<Vec<_>>())?;
        Ok(Config {
            general: raw.general,
            colors,
            bindings,
            floating_mod,
            rules,
            autostart: raw.autostart,
            workspace_outputs,
            keyboard: raw.keyboard,
            inputs: raw.input,
            modes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap();
        assert!(c.bindings.len() > 30);
        let ret = c.bindings.iter().find(|b| b.command == Command::Spawn("foot".into())).unwrap();
        assert_eq!(ret.mods, Modifiers::Mod4);
        assert_eq!(ret.keysym, 0xff0d); // Return
    }

    #[test]
    fn colors() {
        assert_eq!(Color::parse("#ff6b00").unwrap(), Color([0xff, 0x6b, 0, 0xff]));
        assert_eq!(Color::parse("#ffffff80").unwrap().river()[3], 0x8080_8080);
        assert!(Color::parse("#fff").is_err());
    }

    #[test]
    fn bindings_and_rules() {
        let c = Config::parse(
            r##"
            [bindings]
            "Super+Shift+h" = "move left"
            "locked:XF86AudioMute" = "spawn pamixer -t"
            [[rules]]
            app_id = "pavu.*"
            floating = true
            size_percent = [40, 30]
            border_width = 1
            "##,
        )
        .unwrap();
        let b = c.bindings.iter().find(|b| b.command == Command::Move(chaophraya_tree::Dir::Left)).unwrap();
        assert_eq!(b.mods, Modifiers::Mod4 | Modifiers::Shift);
        assert_eq!(b.keysym, 0x68);
        assert!(c.bindings.iter().any(|b| b.locked));
        assert!(c.rules[0].matches("pavucontrol", "x"));
        assert!(!c.rules[0].matches("xpavucontrol", "x"));
        assert_eq!(c.rules[0].size, Some(Size::Percent(40, 30)));
        assert_eq!(c.rules[0].border_width, Some(1));
        assert!(Config::parse("[bindings]\n\"Super+Nope\" = \"kill\"").is_err());
        assert!(Config::parse("[general]\nbogus = 1").is_err());
    }

    #[test]
    fn modes() {
        let c = Config::parse(
            r##"
            [bindings]
            "Super+Tab" = "mode tags"
            [modes.tags]
            "Escape" = "mode default"
            "1" = "view toggle 1"
            "##,
        )
        .unwrap();
        assert_eq!(c.modes, vec!["tags".to_string()]);
        let one = c.bindings.iter().find(|b| b.command == Command::ViewToggle(Some(1))).unwrap();
        assert_eq!(one.mode.as_deref(), Some("tags"));
        assert!(c.bindings.iter().find(|b| b.command == Command::Mode("tags".into())).unwrap().mode.is_none());
        assert!(Config::parse("[bindings]\n\"Super+Tab\" = \"mode nope\"").is_err());
        assert!(Config::parse("[modes.default]\n\"a\" = \"kill\"").is_err());
    }
}
