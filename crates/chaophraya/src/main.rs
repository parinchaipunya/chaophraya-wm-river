//! chaophraya: a sway-like tiling window manager for river 0.5.

mod command;
mod config;
mod deco;
mod input;
mod ipc;
mod protocol;
mod wm;

use std::os::fd::AsRawFd;
use std::process::ExitCode;

use wayland_client::Connection;

use crate::config::Config;
use crate::wm::State;

const USAGE: &str = "\
usage:
  chaophraya                      run the window manager (start it with: river -c chaophraya)
  chaophraya msg <command...>     run a command, e.g. `chaophraya msg workspace 3`
  chaophraya subscribe            print a JSON state line on every change
  chaophraya waybar workspace N   JSON lines for a waybar custom module (also: title, layout)
  chaophraya check-config         parse the config and report errors
";

fn run_wm() -> Result<(), String> {
    let config = Config::load()?;
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot connect to Wayland: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::new(config);
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    if let Some(e) = state.exit.take() {
        return Err(e);
    }
    if state.wm.is_none() {
        return Err("river_window_manager_v1 not found (is this river 0.5 or newer?)".into());
    }
    state.load_keymap(&qh);
    let mut server = ipc::Server::bind()?;
    for c in state.config.autostart.exec.clone().iter().chain(state.config.autostart.exec_always.clone().iter()) {
        state.spawn(c);
    }

    loop {
        queue.dispatch_pending(&mut state).map_err(|e| e.to_string())?;
        server.flush(&mut state);
        if let Some(e) = state.exit.take() {
            return if e.is_empty() { Ok(()) } else { Err(e) };
        }
        conn.flush().map_err(|e| e.to_string())?;
        state.reap();

        let Some(guard) = queue.prepare_read() else { continue };
        let mut fds: Vec<libc::pollfd> = std::iter::once(guard.connection_fd().as_raw_fd())
            .chain(server.fds())
            .map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 })
            .collect();
        // Wake up now and then to reap children even when idle.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 1000) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("poll: {err}"));
            }
        }
        if fds[0].revents != 0 {
            if let Err(e) = guard.read() {
                if !matches!(&e, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
                    return Err(format!("Wayland connection lost: {e}"));
                }
            }
        } else {
            drop(guard);
        }
        if server.poll(&mut state) {
            if let Some(wm) = &state.wm {
                wm.manage_dirty();
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => run_wm(),
        Some("msg") => ipc::msg(&args[1..]),
        Some("subscribe") => ipc::subscribe(),
        Some("waybar") => ipc::waybar(&args[1..]),
        Some("check-config") => Config::load().map(|c| {
            println!("{}: ok ({} bindings, {} rules)", Config::path().display(), c.bindings.len(), c.rules.len())
        }),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown argument '{other}'\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chaophraya: {e}");
            ExitCode::FAILURE
        }
    }
}
