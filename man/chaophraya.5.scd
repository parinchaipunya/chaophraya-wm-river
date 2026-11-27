chaophraya(5)

# NAME

chaophraya - configuration file for the chaophraya window manager

# SYNOPSIS

_$XDG_CONFIG_HOME/chaophraya/config.toml_ (default
_~/.config/chaophraya/config.toml_)

# DESCRIPTION

*chaophraya*(1) reads its configuration from a TOML file. If the file doesn't
exist, a built-in default is used; the same default ships as
_default-config.toml_ in the source tree and is a good starting point.

The file has these sections, all optional:

[[ *Section*
:< *Contents*
|  *[general]*
:  borders, gaps, title bars, focus and output behaviour
|  *[colors]*
:  border and title bar colours
|  *[keyboard]*
:  keyboard layout and key repeat
|  *[[input]]*
:  libinput settings per device (repeatable)
|  *[bindings]*
:  key bindings
|  *[modes.*_name_*]*
:  key bindings of a binding mode (repeatable, one per mode)
|  *[[rules]]*
:  window rules (repeatable)
|  *[workspace_outputs]*
:  which output a workspace opens on
|  *[autostart]*
:  programs started with the session

Unknown sections and keys are errors, so typos don't go unnoticed. Check a file
with *chaophraya check-config*, and apply it with the *reload* command (bound
to Super+Shift+r in the default configuration). A file that fails to load is
rejected and the running configuration is kept.

A reload applies everything except *[autostart]* *exec*, which only runs at
startup. Keyboard and input settings are re-sent to river; new rules apply to
windows opened from then on.

# GENERAL

*border_width* = _integer_
	Border width in pixels. Default: *2*.

*gaps_inner* = _integer_
	Gap between tiled windows, in pixels. Default: *0*.

*gaps_outer* = _integer_
	Gap between tiled windows and the edges of the output, in pixels.
	Default: *0*.

*focus_follows_mouse* = _boolean_
	Focus the window under the cursor when the cursor moves onto it.
	Default: *true*.

*floating_modifier* = _string_
	Modifier for mouse window management: held with the left button it drags
	floating windows; with the right button it resizes floating windows or, on
	tiled windows, moves the split edge nearest the cursor. Modifier names
	are those of *BINDINGS*; join several with *+*. Default: *"Super"*.

*title_height* = _integer_
	Height of title bars in stacking and tabbed containers, in pixels.
	Default: *20*.

*font* = _string_
	Title bar font as "_family_ _size_", the size in points.
	Default: *"Noto Sans 10"*.

*title_format* = _string_
	Title bar text. *{title}* and *{app_id}* are replaced by the window's
	title and app_id. Default: *"{title}"*.

*mouse_warping* = *"output"*|*"container"*|*"none"*
	Where the cursor goes when a key or command moves focus to another output
	(as with sway's *mouse_warping*): to the centre of the output, to the
	focused window, or nowhere. Default: *"output"*.

*park_outputs_except* = [_names_...]
	Park every output except the named ones as soon as it appears, whether it
	was connected before the session started or during it. A parked output
	stays on, but it shows no workspace, and focus, window moves and cursor
	warps skip it. *chaophraya msg output* _name_ *unpark* makes it usable.
	If the only connected outputs are parked, one is unparked so there is
	always a usable screen. An empty list parks nothing. Default: *[]*.

	This is the way to keep external monitors "off by default" on river.
	River switches every newly connected output on, also one that is just
	reconnecting, and some monitors disconnect whenever their signal stops.
	Switching such a monitor off with *wlr-randr --off* starts a loop: it
	disconnects, reconnects and is switched back on. A parked monitor stays on
	(showing black, apart from status bars drawn on it) and never bounces.
	Place a parked output away from the others, e.g. *wlr-randr --output*
	_name_ *--pos 20000,0*, so the cursor can't reach it.

	Typical laptop setting: *park_outputs_except = ["eDP-1"]*.

# COLORS

Colours are *"#rrggbb"* or *"#rrggbbaa"*.

[[ *Key*
:< *Used for*
:< *Default*
|  *focused*
:  border of the focused window
:  *#ff6b00*
|  *unfocused*
:  border of other windows
:  *#505050*
|  *urgent*
:  border of a window marked urgent
:  *#d08770*
|  *focused_title*, *focused_title_text*
:  title bar of the focused window
:  *#ff6b00*, *#ececec*
|  *active_title*, *active_title_text*
:  title bar of the selected window of a container that doesn't have focus
:  *#5f5f5f*, *#ececec*
|  *unfocused_title*, *unfocused_title_text*
:  other title bars
:  *#505050*, *#a5a5a5*
|  *title_separator*
:  line between title bars
:  *#2e3440*

# KEYBOARD

*layout* = _string_
	xkb layouts, comma separated, e.g. *"us,th"*. Default: *"us"*.

*variant* = _string_, *model* = _string_
	xkb variants (comma separated, one per layout) and keyboard model.
	Default: empty.

*options* = _string_
	xkb options, comma separated, e.g. *"grp:ctrl_space_toggle,compose:caps"*.
	See *xkeyboard-config*(7). Default: empty.

*repeat_rate* = _integer_
	Key repeat rate, in keys per second. Default: *25*.

*repeat_delay* = _integer_
	Delay before keys start repeating, in milliseconds. Default: *600*.

# INPUT

Each *[[input]]* table applies libinput settings to the devices it matches.
Every device gets the settings of all tables that match it, later tables
overriding earlier ones. Settings a table leaves out are left as they are.

*match* = _string_
	Which devices: *"touchpad"*, *"pointer"* (mice, touchpads, trackpoints),
	*"keyboard"*, *"\*"* (all), or otherwise a case-insensitive part of the
	device name, e.g. *"TrackPoint"*. Run chaophraya with *CHAOPHRAYA_DEBUG=1*
	to log device names.

*tap* = _boolean_
	Tap to click.

*natural_scroll* = _boolean_
	Reverse the scroll direction.

*dwt* = _boolean_
	Disable the touchpad while typing.

*middle_emulation* = _boolean_
	Pressing the left and right buttons together clicks the middle button.

*accel_speed* = _number_
	Pointer acceleration from *-1.0* (slowest) to *1.0* (fastest).

*accel_profile* = *"flat"*|*"adaptive"*
	Acceleration profile.

# BINDINGS

Each entry of *[bindings]* maps a key combination to a command:

	"Super+Shift+Return" = "spawn foot"

The key combination is modifiers and a key joined with *+*. Modifiers are
*Super* (also *Mod4*, *Logo*, *Win*), *Alt* (*Mod1*), *Ctrl* (*Control*),
*Shift*, *Mod3* and *Mod5* (*AltGr*), case insensitive. The key is an xkb keysym
name: a letter or digit (*a*, *1*), or a name such as *Return*, *space*,
*minus*, *Print*, *F7*, *Left* or *XF86AudioRaiseVolume*; see
_/usr/include/xkbcommon/xkbcommon-keysyms.h_ for all names (without the
*XKB_KEY_* prefix). Letter case doesn't matter: use *Shift* for shifted keys.

Keys are matched on the first keyboard layout, so bindings keep working while
another layout (e.g. Thai) is active.

Prefix a key combination with *locked:* to keep the binding active while the
session is locked, e.g. for media keys:

	"locked:XF86AudioMute" = "spawn pamixer --toggle-mute"

All other bindings are disabled while the session is locked.

The commands are those of *chaophraya*(1), *COMMANDS*. A *spawn* command runs
with *sh -c*, so pipes and *$(...)* work. When a bound command fails (e.g. a
*reload* of a broken file), the error is shown with *notify-send*(1).

# MODES

A *[modes.*_name_*]* table defines a binding mode, as sway's *mode* blocks.
Its entries have the same form as *[bindings]*. The command *mode* _name_
switches to it: from then on only that table's bindings are active, until
*mode default* returns to *[bindings]*. Include a way out of every mode, e.g.
*Escape* = *"mode default"*. The current mode is reported in the state and by
*chaophraya waybar mode*.

Inside a mode, keys usually need no modifier. A key typed with Shift (such as
*\** on Shift+8) is safest bound both with and without *Shift+*; only one of
them can match a key press. While the session is locked, only the default
mode's *locked:* bindings are active.

A binding to *mode* _name_ without a matching table is an error, and
*default* can't be used as a table name.

Example: river-classic's tags management, showing other workspaces beside the
current one (see *COMBINED VIEWS* in *chaophraya*(1)):

```
[bindings]
"Super+Tab" = "mode tags"

[modes.tags]
"Super+Tab" = "mode default"
"Escape" = "mode default"
"1" = "view toggle 1"
"2" = "view toggle 2"
"0" = "view toggle 10"
"minus" = "scratchpad show all"
"asterisk" = "view toggle all"
"Shift+asterisk" = "view toggle all"
```

# RULES

Each *[[rules]]* table matches new windows by *app_id* and/or *title* and sets
properties on them. Rules apply when a window opens; every matching rule
applies, in file order, so later rules override earlier ones.

*app_id* = _regex_, *title* = _regex_
	Regular expressions that must match the whole app_id or title (they are
	anchored at both ends). *"firefox|Chromium"* matches either app;
	*"Picture.\*"* matches titles starting with "Picture". A rule with both
	keys needs both to match; a rule with neither matches every window. Use
	*chaophraya msg get_windows* to find a window's app_id and title. X11
	(Xwayland) windows report their class as app_id.

	sway's *window_role*, *window_type*, *instance* and *shell* criteria have
	no equivalent: river doesn't report these properties.

*floating* = _boolean_
	Open floating (*true*) or tiled (*false*). Without a rule, windows that
	have a parent (dialogs, file pickers, X11 transient windows) or a fixed
	size float, and everything else is tiled. New floating windows are
	centred on their output.

*sticky* = _boolean_
	Open sticky: visible on every workspace of its output.

*workspace* = _1-10_
	Open on this workspace.

*size* = [_width_, _height_]
	Floating size in pixels.

*size_percent* = [_width_, _height_]
	Floating size in percent of the output. *size* takes precedence.

*border_width* = _integer_
	Border width of this window in pixels, overriding *border_width* in
	*[general]* (as sway's *border pixel* _n_).

*scratchpad* = _boolean_
	Send the window straight to the scratchpad.

*kill* = _boolean_
	Close the window as soon as it appears (e.g. unwanted sharing indicators).

*inhibit_idle* = *"fullscreen"*|*"visible"*|*"focus"*|*"open"*
	Keep the session from going idle while the window is fullscreen and
	visible, visible, focused, or open (as sway's *inhibit_idle*). River
	ignores idle inhibitors from the window manager, so chaophraya reports
	this as *idle_inhibited* in its state, and the idle script (e.g. the
	_idle-lock.sh_ shipped with chaophraya, run by *swayidle*(1)) must check it
	before locking.

# WORKSPACE OUTPUTS

*[workspace_outputs]* maps workspace numbers (as strings) to output names. A
workspace opens on its output when it is shown and isn't already visible; if
that output isn't connected or usable, it opens on the focused output like any
other workspace.

	\[workspace_outputs]++
"1" = "eDP-1"++
"10" = "HDMI-A-1"

The command *workspace* _n_ *output* _name_ overrides this during the session.

# AUTOSTART

*exec* = [_commands_...]
	Shell commands run once, when chaophraya starts.

*exec_always* = [_commands_...]
	Shell commands run at start and again on every *reload*. Make them safe to
	run twice, e.g. *"pkill -x gammastep; exec gammastep"*.

Commands run with *sh -c* and inherit *CHAOPHRAYA_SOCK*, *WAYLAND_DISPLAY* and
the rest of the session's environment. Their output goes to chaophraya's
standard error.

# EXAMPLE

```
[general]
border_width = 2
title_format = "{title} ({app_id})"
park_outputs_except = ["eDP-1"]

[colors]
focused = "#ff6b00"

[keyboard]
layout = "us,th"
options = "grp:ctrl_space_toggle"

[[input]]
match = "touchpad"
tap = true
accel_speed = 0.5

[bindings]
"Super+Return" = "spawn foot"
"Super+q" = "kill"
"Super+1" = "workspace 1"
"Super+Shift+1" = "move to workspace 1"
"Super+s" = "layout stacking"
"Super+w" = "layout tabbed"
"Super+e" = "layout toggle split"
"Super+y" = "sticky toggle"
"Super+Shift+r" = "reload"
"locked:XF86AudioMute" = "spawn pamixer --toggle-mute"

[[rules]]
app_id = "pavucontrol"
floating = true
size_percent = [40, 30]

[[rules]]
app_id = "org.mozilla.Thunderbird"
workspace = 9

[workspace_outputs]
"1" = "eDP-1"

[autostart]
exec = ["dunst"]
exec_always = ["~/.config/waybar/launch.sh"]
```

# SEE ALSO

*chaophraya*(1), *river*(1), *xkeyboard-config*(7), *libinput*(4),
*wlr-randr*(1), *sway*(5)
