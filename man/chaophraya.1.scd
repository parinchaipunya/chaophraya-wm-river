chaophraya(1)

# NAME

chaophraya - a sway-like tiling window manager for river

# SYNOPSIS

*river -c chaophraya*

*chaophraya msg* _command_...

*chaophraya subscribe*

*chaophraya waybar* *workspace* _N_ | *title* | *layout* | *keyboard* | *mode*

*chaophraya check-config*

*chaophraya* *-h* | *--help*

# DESCRIPTION

chaophraya is a window manager for the river Wayland compositor (0.5 or
newer). River draws the screen and handles input devices and monitors;
chaophraya decides where windows go, draws borders and title bars, and handles
key bindings, using river's _river-window-management-v1_ protocol. It runs as a
client of river, started with *river -c chaophraya*.

It replicates sway's workflow:

- *A sway-style tiling tree.* A lone window fills the workspace. The second
  window splits it left/right; after that, a new window splits the focused
  window top/bottom (or joins its container). *splith*, *splitv* and *layout
  toggle split* change this, as in sway.
- *Stacking and tabbed layouts* with title bars drawn by the window manager.
  Clicking a title bar focuses that window.
- *Workspaces 1-10* that behave like sway's: each output shows one workspace,
  each workspace remembers its focused window, and workspaces can be pinned to
  outputs.
- *Sticky windows*, tiled or floating, that appear on every workspace of their
  output. A sticky tiled window always sits in the same slot: at the bottom of
  the right-most column.
- Floating windows, fullscreen, a sway-style scratchpad, and window rules (see
  *chaophraya*(5)).
- *No built-in bar.* Status bars such as waybar get the state over IPC (see
  *IPC* below).

Without arguments, chaophraya runs as the window manager. It must be started
by river: when river is started without *-c*, it runs _~/.config/river/init_
instead.

The configuration is read from _$XDG_CONFIG_HOME/chaophraya/config.toml_
(default _~/.config/chaophraya/config.toml_), or the built-in default if that
file doesn't exist. See *chaophraya*(5).

# OPTIONS

*msg* _command_...
	Send _command_ to the running window manager and print the reply. The words
	are joined with spaces, so *chaophraya msg workspace 3* and *chaophraya msg
	"workspace 3"* are the same. Commands are listed under *COMMANDS*. Besides
	those, *msg* accepts two queries:

	*get_state*
		Print the current state as one JSON object (see *IPC*).

	*get_windows*
		Print all windows as a JSON array. Each has *id*, *title*, *app_id*,
		*workspace* (absent in the scratchpad), *floating*, *sticky*,
		*fullscreen*, *focused*, *visible*, *urgent* and *rect* ([x, y, width,
		height] in global coordinates). Ids can be used with *focus id*.

	*msg* exits with status 0 if the command was carried out, otherwise it
	prints the error and exits with status 1.

*subscribe*
	Print the state as a JSON line (see *IPC*) now and every time it changes,
	until the window manager exits.

*waybar* *workspace* _N_ | *title* | *layout* | *keyboard* | *mode*
	Print JSON lines for a waybar custom module with *"return-type": "json"*,
	one per change. Each line has *text*, *tooltip* and *class*:

	*workspace* _N_
		Text _N_, with class *focused*, *urgent*, *visible* or *occupied*. The
		text is empty (hiding the module) while the workspace is neither shown
		nor holds windows.

	*title*
		Title of the focused window; the tooltip is its app_id and the class is
		*sticky* when it is sticky.

	*layout*
		*[H]*, *[V]*, *[S]*, *[T]* or *[F]* for the focused window's container:
		splith, splitv, stacking, tabbed or floating.

	*keyboard*
		Active keyboard layout code, e.g. *US* (class *us*).

	*mode*
		Name of the current binding mode; empty (hidden) in the default
		mode.

*check-config*
	Parse the configuration file and report the first error, with its line
	number for TOML syntax errors. Exits with status 1 on errors.

*-h*, *--help*
	Show a usage summary.

# COMMANDS

Key bindings (see *chaophraya*(5)) and *chaophraya msg* use the same commands,
modelled on sway's.

## Programs and session

*spawn* _shell command_, *exec* _shell command_
	Run _shell command_ with *sh -c*. A leading *--no-startup-id* is ignored.

*reload*
	Reload the configuration file and run its *exec_always* commands again. An
	invalid file is rejected and the running configuration is kept; the error
	is shown as a notification when the command came from a key binding, or
	printed by *chaophraya msg*.

*exit*
	End the window manager, and with it the river session.

*nop*
	Do nothing.

## Windows

*kill*, *close*
	Ask the focused window to close.

*focus* *left*|*right*|*up*|*down*
	Focus the neighbouring window. At the edge of a workspace, focus moves to
	the workspace on the next output in that direction.

*focus mode_toggle*
	Switch focus between tiled and floating windows.

*focus id* _n_
	Focus the window with id _n_ (from *get_windows*), showing its workspace
	if necessary. Fails for hidden scratchpad windows.

*move* *left*|*right*|*up*|*down*
	Move the focused window in the tree, or a floating window by 20 pixels. At
	the edge of a workspace, the window moves to the next output.

*move* [*container*|*window*] [*to*] *workspace* [*number*] _n_
	Send the focused window to workspace _n_ (1-10).

*fullscreen*
	Toggle fullscreen for the focused window.

*floating toggle*
	Toggle between tiled and floating.

*sticky toggle*
	Toggle sticky: the window stays visible on every workspace of its output.

*resize* *grow*|*shrink* *width*|*height* _n_ [*px*]
	Make the focused window _n_ pixels bigger or smaller.

## Layout

*splith*, *splitv*, *split* *h*|*v*|*horizontal*|*vertical*
	Open the next window beside (*h*) or below (*v*) the focused one.

*layout* *splith*|*splitv*|*stacking*|*tabbed*
	Set the layout of the focused window's container.

*layout toggle split*, *layout toggle*
	Swap the split direction, or leave stacking/tabbed.

## Workspaces and outputs

*workspace* [*number*] _n_
	Show workspace _n_ (1-10). If it is already visible on another output,
	focus that output. Otherwise it opens on the output it is pinned to, or on
	the focused output.

*view toggle* _n_
	Show workspace _n_ beside the current one on the focused output, or hide
	it again: a combined view (see *COMBINED VIEWS*). Hiding the workspace
	that has the focus moves the focus to the next one shown. Fails if _n_ is
	shown on another output.

*view toggle all*
	Show every workspace that has windows (except those shown on other
	outputs and the scratchpad) beside the current one; if they are all
	shown already, hide all but the focused one.

*workspace* _n_ *output* _name_
	Open workspace _n_ on output _name_ from now on. This overrides
	*[workspace_outputs]* in the configuration until the session ends.

*move workspace to output* _name_, *move workspace output* _name_
	Move the focused workspace to output _name_.

*output* _name_ *park*|*unpark*
	Park or unpark output _name_. A parked output stays on but shows no
	workspace, and focus, window moves and cursor warps skip it. See
	*park_outputs_except* in *chaophraya*(5). Parking the only usable output
	is refused.

## Scratchpad

*move scratchpad*
	Hide the focused window in the scratchpad (it becomes floating).

*scratchpad show*
	Like sway: if the focused window is a shown scratchpad window, hide it
	again; if a scratchpad window is shown elsewhere, bring it here; otherwise
	show the next hidden one on the current workspace. Repeating the command
	cycles through all scratchpad windows. A shown scratchpad window that is
	made tiled (*floating toggle*) leaves the scratchpad.

*scratchpad show all*
	Show every hidden scratchpad window on the current workspace, slightly
	cascaded. If none is hidden, hide all shown scratchpad windows.

## Other

*mode* _name_
	Switch to binding mode _name_, a *[modes.*_name_*]* table of the
	configuration, or back to *default*. Only the current mode's bindings
	are active. See *chaophraya*(5).

*keyboard layout* _n_|*next*
	Switch to keyboard layout _n_ (counting from 0 in *[keyboard]* *layout*),
	or to the next one.

*urgent app_id* _name_
	Mark the hidden windows of app _name_ urgent: their workspace gets the
	*urgent* class in the state and in *waybar workspace*. River doesn't report
	urgency to the window manager, so a notification daemon or a link-opening
	script runs this. The mark clears when the window is focused.

# COMBINED VIEWS

An output normally shows one workspace. *view toggle* adds other workspaces
to it, river-classic style: the output is divided into one region per shown
workspace, in workspace order, side by side (a grid for more than three).
Each workspace keeps its own tree and layout, so hiding it again leaves
everything as it was.

The workspace of the focused window is the active one: new windows open
there, and status bars show it as focused; the other shown workspaces are
reported as visible. *focus* and *move* cross from one region to the next
before crossing to another output. Sticky windows stay in the region of the
workspace they are on.

The combined view lasts until the workspace changes: *workspace* _n_ shows
_n_ alone again (and the sticky windows come along).

# MOUSE

With the *floating_modifier* key held (default Super):

- *left button drag* moves a floating window;
- *right button drag* resizes a floating window or, on a tiled window, moves
  the split edge nearest to the cursor.

Clicking a title bar of a stacked or tabbed container focuses that window.
With *focus_follows_mouse*, focus follows the cursor between windows.

# MULTIPLE OUTPUTS

Each output shows one workspace. Outputs are arranged with an output
configuration tool such as *wlr-randr*(1) or *kanshi*(1); chaophraya follows the
positions river reports.

An output placed exactly on top of another (same position and size) is treated
as its mirror: it shows the same area and gets no workspace of its own. An
output listed in *park_outputs_except* is preferred as the one that keeps the
workspace.

When a command moves focus to another output, the cursor is moved too (see
*mouse_warping* in *chaophraya*(5)).

River switches a newly connected output on, also when it is reconnecting after
being switched off. Some monitors disconnect when their signal stops, so
switching them off makes them reconnect, and river switches them back on, in a
loop. Parking (*output* _name_ *park*, *park_outputs_except*) keeps such a
monitor on but unused instead.

# IPC

chaophraya listens on the Unix socket
_$XDG_RUNTIME_DIR/chaophraya.$WAYLAND_DISPLAY.sock_. Programs it starts get the
path in *$CHAOPHRAYA_SOCK*, which *msg*, *subscribe* and *waybar* use when
set.

The protocol is line based. A client sends a command line (see *COMMANDS*) and
gets one reply line: *ok* or *error:* _message_. The line *get_state* or
*get_windows* is answered with one JSON line. The line *subscribe* turns the
connection into a stream of state lines, one per change.

A state object has these fields:

*focused_output*
	Name of the focused output.

*workspaces*
	Workspaces that are shown or hold windows, each with *num*, *output* (if
	shown), *visible*, *focused*, *windows* (count) and *urgent*.

*title*, *app_id*
	Of the focused window.

*layout*
	Layout of the focused window's container: *splith*, *splitv*,
	*stacking*, *tabbed* or *floating*.

*sticky*
	Whether the focused window is sticky.

*keyboard_layout*
	Active layout code, e.g. *US*.

*idle_inhibited*
	A window's *inhibit_idle* rule is currently active (see *chaophraya*(5)).

*pointer*
	Cursor position [x, y] in global coordinates.

*mode*
	Current binding mode, *default* or a *[modes.*_name_*]* table.

# ENVIRONMENT

*XDG_CONFIG_HOME*
	Base directory of the configuration file (default _~/.config_).

*XDG_RUNTIME_DIR*, *WAYLAND_DISPLAY*
	Determine the IPC socket path. The window manager always derives its
	socket from these, never from an inherited *CHAOPHRAYA_SOCK*.

*CHAOPHRAYA_SOCK*
	Socket used by *msg*, *subscribe* and *waybar*. Set by the window manager
	for every program it starts. Unset it to talk to another session, e.g. a
	nested one.

*CHAOPHRAYA_DEBUG*
	When set, log input device details to standard error.

# FILES

_$XDG_CONFIG_HOME/chaophraya/config.toml_
	Configuration file, see *chaophraya*(5).

_$XDG_RUNTIME_DIR/chaophraya.$WAYLAND_DISPLAY.sock_
	IPC socket.

# EXIT STATUS

0 on success; 1 if the window manager stopped with an error, or a client
command failed.

# EXAMPLES

Start a session from a TTY, logging errors:

	river -c 'exec chaophraya 2>>~/.local/state/chaophraya/chaophraya.log'

Switch workspace and show the result:

	chaophraya msg workspace 3++
chaophraya msg get_state | jq .workspaces

A waybar module for workspace 1:

```
"custom/ws1": {
    "exec": "chaophraya waybar workspace 1",
    "return-type": "json",
    "on-click": "chaophraya msg workspace 1"
}
```

Use an external monitor parked by *park_outputs_except*:

	wlr-randr --output HDMI-A-1 --pos 1920,0++
chaophraya msg output HDMI-A-1 unpark

# LIMITATIONS

These come from river 0.5's protocols:

- River reports no window urgency; see *urgent app_id*.
- Pointer bindings are buttons only, so there are no scroll-wheel bindings.
- River ignores idle inhibitors on the window manager's surfaces, so
  *inhibit_idle* rules are reported in the state for an idle script to honour.

# SEE ALSO

*chaophraya*(5), *river*(1), *wlr-randr*(1), *waybar*(5), *sway*(5)
