//! Generated bindings for the river 0.5 protocols chaophraya uses.
#![allow(non_upper_case_globals, non_camel_case_types, dead_code, unused_imports, clippy::all)]

pub use wayland_client;
pub use wayland_client::protocol::*;

mod interfaces {
    pub(super) mod rwm {
        pub use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/river-window-management-v1.xml");
    }
    pub(super) mod rxkb {
        use super::rwm::*;
        wayland_scanner::generate_interfaces!("protocol/river-xkb-bindings-v1.xml");
    }
    pub(super) mod rls {
        use super::rwm::*;
        wayland_scanner::generate_interfaces!("protocol/river-layer-shell-v1.xml");
    }
    pub(super) mod rim {
        pub use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/river-input-management-v1.xml");
    }
    pub(super) mod rxc {
        use super::rim::*;
        wayland_scanner::generate_interfaces!("protocol/river-xkb-config-v1.xml");
    }
    pub(super) mod rli {
        use super::rim::*;
        wayland_scanner::generate_interfaces!("protocol/river-libinput-config-v1.xml");
    }
}

use self::interfaces::rim::*;
use self::interfaces::rli::*;
use self::interfaces::rls::*;
use self::interfaces::rxc::*;
use self::interfaces::rwm::*;
use self::interfaces::rxkb::*;
wayland_scanner::generate_client_code!("protocol/river-window-management-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-xkb-bindings-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-layer-shell-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-input-management-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-xkb-config-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-libinput-config-v1.xml");
