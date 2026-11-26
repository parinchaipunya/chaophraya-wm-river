//! rdeck: river-classic layout generator (river-layout-v3) built on
//! `rdeck-layout`. Settings are kept per output and per tag set.

use std::collections::HashMap;
use std::process::ExitCode;

use rdeck_layout::{layout, Params};
use wayland_client::protocol::{wl_output::WlOutput, wl_registry};
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle};

#[allow(non_upper_case_globals, non_camel_case_types, dead_code, clippy::all)]
mod protocol {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/river-layout-v3.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocol/river-layout-v3.xml");
}

use protocol::river_layout_manager_v3::RiverLayoutManagerV3;
use protocol::river_layout_v3::{self, RiverLayoutV3};

const USAGE: &str = "\
usage: rdeck [options]

  -namespace NAME        layout namespace for `riverctl default-layout` (default: rdeck)
  -main-location LOC     left, right, top, bottom, deck or monocle (default: left)
  -main-count N          windows in the main area (default: 1)
  -main-ratio F          main area fraction, 0.1-0.9 (default: 0.6)
  -inner-gaps N          gap between windows in px (default: 6)
  -outer-gaps N          gap to the screen edge in px (default: 6)
  -no-smart-gaps         keep gaps when only one window is visible
  -deck-offset N         vertical shift between deck cards in px (default: 0)

Runtime commands (riverctl send-layout-cmd <namespace> \"...\"), applied to the
focused output's current tags. Numbers prefixed with + or - are relative:

  main-location LOC | main-location-cycle LOC,LOC,...
  main-count N | main-ratio F | inner-gaps N | outer-gaps N | deck-offset N
  smart-gaps on|off|toggle
";

struct Output {
    global: u32,
    wl: WlOutput,
    layout: Option<RiverLayoutV3>,
    /// Settings per tag bitmask, created from the defaults on first use.
    params: HashMap<u32, Params>,
    /// Tags that the next user_command applies to (from user_command_tags).
    command_tags: u32,
}

struct App {
    defaults: Params,
    namespace: String,
    manager: Option<RiverLayoutManagerV3>,
    outputs: Vec<Output>,
    fatal: Option<String>,
}

impl App {
    fn attach_layouts(&mut self, qh: &QueueHandle<Self>) {
        let Some(manager) = &self.manager else { return };
        for out in self.outputs.iter_mut().filter(|o| o.layout.is_none()) {
            out.layout = Some(manager.get_layout(&out.wl, self.namespace.clone(), qh, out.global));
        }
    }

    fn output(&mut self, global: u32) -> Option<&mut Output> {
        self.outputs.iter_mut().find(|o| o.global == global)
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        app: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } => match interface.as_str() {
                "river_layout_manager_v3" => {
                    app.manager = Some(registry.bind(name, version.min(2), qh, ()));
                    app.attach_layouts(qh);
                }
                "wl_output" => {
                    let wl = registry.bind(name, version.min(4), qh, ());
                    app.outputs.push(Output {
                        global: name,
                        wl,
                        layout: None,
                        params: HashMap::new(),
                        command_tags: 0,
                    });
                    app.attach_layouts(qh);
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = app.outputs.iter().position(|o| o.global == name) {
                    let out = app.outputs.remove(i);
                    if let Some(l) = out.layout {
                        l.destroy();
                    }
                    if out.wl.version() >= 3 {
                        out.wl.release();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverLayoutV3, u32> for App {
    fn event(
        app: &mut Self,
        layout_obj: &RiverLayoutV3,
        event: river_layout_v3::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let defaults = app.defaults.clone();
        match event {
            river_layout_v3::Event::NamespaceInUse => {
                app.fatal = Some(format!(
                    "namespace '{}' is already in use (is another rdeck running?)",
                    app.namespace
                ));
            }
            river_layout_v3::Event::LayoutDemand { view_count, usable_width, usable_height, tags, serial } => {
                let Some(out) = app.output(*global) else { return };
                let p = out.params.entry(tags).or_insert(defaults);
                for r in layout(p, view_count, usable_width, usable_height) {
                    layout_obj.push_view_dimensions(r.x, r.y, r.w, r.h, serial);
                }
                layout_obj.commit(p.label().to_owned(), serial);
            }
            river_layout_v3::Event::UserCommandTags { tags } => {
                if let Some(out) = app.output(*global) {
                    out.command_tags = tags;
                }
            }
            river_layout_v3::Event::UserCommand { command } => {
                let Some(out) = app.output(*global) else { return };
                let p = out.params.entry(out.command_tags).or_insert(defaults);
                if let Err(e) = p.apply_command(&command) {
                    eprintln!("rdeck: {e}");
                }
                // river sends a fresh layout_demand after this when relevant.
            }
        }
    }
}

delegate_noop!(App: ignore WlOutput);
delegate_noop!(App: ignore RiverLayoutManagerV3);

fn parse_args() -> Result<(Params, String), String> {
    let mut params = Params::default();
    let mut namespace = "rdeck".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        // Accept both -flag and --flag.
        let name = flag.trim_start_matches('-');
        match name {
            "h" | "help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "no-smart-gaps" => params.smart_gaps = false,
            "namespace" => namespace = args.next().ok_or("-namespace needs a value")?,
            "main-location" | "main-count" | "main-ratio" | "inner-gaps" | "outer-gaps" | "deck-offset" => {
                let value = args.next().ok_or_else(|| format!("-{name} needs a value"))?;
                // Flags are absolute: refuse a leading sign rather than treat it as relative.
                if value.starts_with('+') || (value.starts_with('-') && name != "main-location") {
                    return Err(format!("-{name}: expected a plain value, got '{value}'"));
                }
                params.apply_command(&format!("{name} {value}"))?;
            }
            _ => return Err(format!("unknown option '{flag}'\n\n{USAGE}")),
        }
    }
    Ok((params, namespace))
}

fn run() -> Result<(), String> {
    let (defaults, namespace) = parse_args()?;
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot connect to Wayland: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut app = App { defaults, namespace, manager: None, outputs: Vec::new(), fatal: None };
    queue.roundtrip(&mut app).map_err(|e| e.to_string())?;
    if app.manager.is_none() {
        return Err("compositor has no river_layout_manager_v3 (not river-classic?)".into());
    }

    loop {
        queue.blocking_dispatch(&mut app).map_err(|e| format!("Wayland connection lost: {e}"))?;
        if let Some(e) = app.fatal.take() {
            return Err(e);
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rdeck: {e}");
            ExitCode::FAILURE
        }
    }
}
