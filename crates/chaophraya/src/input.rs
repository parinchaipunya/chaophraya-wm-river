//! Keyboard layout/repeat and libinput settings (sway's `input` blocks).

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_backend::client::ObjectId;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use crate::config::InputRule;
use crate::protocol::river_input_device_v1::{self, RiverInputDeviceV1};
use crate::protocol::river_input_manager_v1::{self, RiverInputManagerV1};
use crate::protocol::river_libinput_config_v1::{self, RiverLibinputConfigV1};
use crate::protocol::river_libinput_device_v1::{self, RiverLibinputDeviceV1};
use crate::protocol::river_libinput_result_v1::{self, RiverLibinputResultV1};
use crate::protocol::river_xkb_config_v1::{self, RiverXkbConfigV1};
use crate::protocol::river_xkb_keyboard_v1::{self, RiverXkbKeyboardV1};
use crate::protocol::river_xkb_keymap_v1::{self, RiverXkbKeymapV1};
use crate::wm::State;

struct Device {
    proxy: RiverInputDeviceV1,
    name: String,
    keyboard: bool,
    pointer: bool,
}

struct LibinputDevice {
    proxy: RiverLibinputDeviceV1,
    device: Option<ObjectId>,
    tap_fingers: i32,
    applied: bool,
}

#[derive(Default)]
pub struct Input {
    pub manager: Option<RiverInputManagerV1>,
    pub xkb: Option<RiverXkbConfigV1>,
    pub libinput: Option<RiverLibinputConfigV1>,
    devices: HashMap<ObjectId, Device>,
    keyboards: Vec<RiverXkbKeyboardV1>,
    keymap: Option<RiverXkbKeymapV1>,
    keymap_ok: bool,
    lidevs: Vec<LibinputDevice>,
    /// Active layout index from the keyboard.
    pub layout_index: Option<u32>,
    /// Settings should be applied in the next manage sequence.
    pub apply_pending: bool,
}

impl State {
    /// Compile the configured keymap and hand it to river.
    pub fn load_keymap(&mut self, qh: &QueueHandle<State>) {
        let Some(xkb) = self.input.xkb.clone() else { return };
        let k = &self.config.keyboard;
        let ctx = xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);
        let keymap = xkbcommon::xkb::Keymap::new_from_names(
            &ctx,
            "",
            &k.model,
            &k.layout,
            &k.variant,
            Some(k.options.clone()),
            xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
        );
        let Some(keymap) = keymap else {
            eprintln!("chaophraya: cannot compile keymap layout='{}' options='{}'", k.layout, k.options);
            return;
        };
        let mut text = keymap.get_as_string(xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1).into_bytes();
        text.push(0);
        let fd = unsafe { libc::memfd_create(c"chaophraya-keymap".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return;
        }
        let mut file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        if file.write_all(&text).is_err() {
            return;
        }
        if let Some(old) = self.input.keymap.take() {
            old.destroy();
        }
        self.input.keymap_ok = false;
        self.input.keymap =
            Some(xkb.create_keymap(file.as_fd(), river_xkb_config_v1::KeymapFormat::TextV1, qh, ()));
    }

    /// Re-apply repeat and libinput settings (after a reload).
    pub fn reapply_inputs(&mut self, qh: &QueueHandle<State>) {
        for d in self.input.devices.values() {
            if d.keyboard {
                d.proxy.set_repeat_info(self.config.keyboard.repeat_rate, self.config.keyboard.repeat_delay);
            }
        }
        for l in &mut self.input.lidevs {
            l.applied = false;
        }
        self.load_keymap(qh);
        self.apply_libinput(qh);
    }

    /// A device appeared or finished describing itself. Its libinput events
    /// (tap support etc.) arrive *after* the link event and there is no "done"
    /// event, so apply settings in the next manage sequence, once the whole
    /// burst has been read.
    pub fn request_libinput_apply(&mut self) {
        self.input.apply_pending = true;
        if let Some(wm) = &self.wm {
            wm.manage_dirty();
        }
    }

    pub fn apply_libinput(&mut self, qh: &QueueHandle<State>) {
        let rules = self.config.inputs.clone();
        for l in &mut self.input.lidevs {
            if l.applied {
                continue;
            }
            let Some(dev) = l.device.as_ref().and_then(|id| self.input.devices.get(id)) else { continue };
            if dev.name.is_empty() {
                continue;
            }
            for r in rules.iter().filter(|r| rule_matches(r, dev, l.tap_fingers)) {
                apply_rule(&l.proxy, r, qh);
            }
            l.applied = true;
        }
    }

    /// Switch every keyboard to layout `index`, or to the next layout.
    pub fn set_keyboard_layout(&mut self, index: Option<u32>) {
        let count = self.config.keyboard.layout.split(',').count().max(1) as u32;
        let i = index.unwrap_or_else(|| self.input.layout_index.map_or(0, |c| (c + 1) % count));
        for k in &self.input.keyboards {
            k.set_layout_by_index(i as i32);
        }
    }

    /// Short code of the active layout, e.g. "US" or "TH".
    pub fn layout_code(&self) -> Option<String> {
        let i = self.input.layout_index? as usize;
        self.config.keyboard.layout.split(',').nth(i).map(|s| s.trim().to_uppercase())
    }
}

fn rule_matches(r: &InputRule, dev: &Device, tap_fingers: i32) -> bool {
    match r.matches.as_str() {
        "touchpad" => dev.pointer && tap_fingers > 0,
        "pointer" => dev.pointer,
        "keyboard" => dev.keyboard,
        "*" => true,
        name => dev.name.to_lowercase().contains(&name.to_lowercase()),
    }
}

fn apply_rule(d: &RiverLibinputDeviceV1, r: &InputRule, qh: &QueueHandle<State>) {
    use river_libinput_device_v1::*;
    if let Some(on) = r.tap {
        d.set_tap(if on { TapState::Enabled } else { TapState::Disabled }, qh, ());
    }
    if let Some(on) = r.natural_scroll {
        d.set_natural_scroll(if on { NaturalScrollState::Enabled } else { NaturalScrollState::Disabled }, qh, ());
    }
    if let Some(on) = r.dwt {
        d.set_dwt(if on { DwtState::Enabled } else { DwtState::Disabled }, qh, ());
    }
    if let Some(on) = r.middle_emulation {
        let s = if on { MiddleEmulationState::Enabled } else { MiddleEmulationState::Disabled };
        d.set_middle_emulation(s, qh, ());
    }
    if let Some(p) = r.accel_profile.as_deref() {
        let p = match p {
            "flat" => AccelProfile::Flat,
            "adaptive" => AccelProfile::Adaptive,
            _ => AccelProfile::None,
        };
        d.set_accel_profile(p, qh, ());
    }
    if let Some(s) = r.accel_speed {
        d.set_accel_speed(s.clamp(-1.0, 1.0).to_ne_bytes().to_vec(), qh, ());
    }
}

impl Dispatch<RiverInputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &RiverInputManagerV1,
        event: river_input_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_input_manager_v1::Event::InputDevice { id } = event {
            state.input.devices.insert(id.id(), Device { proxy: id, name: String::new(), keyboard: false, pointer: false });
        }
    }

    wayland_client::event_created_child!(State, RiverInputManagerV1, [
        river_input_manager_v1::EVT_INPUT_DEVICE_OPCODE => (RiverInputDeviceV1, ())
    ]);
}

impl Dispatch<RiverInputDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverInputDeviceV1,
        event: river_input_device_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use river_input_device_v1::{Event, Type};
        let (rate, delay) = (state.config.keyboard.repeat_rate, state.config.keyboard.repeat_delay);
        let Some(d) = state.input.devices.get_mut(&proxy.id()) else { return };
        match event {
            Event::Removed => {
                proxy.destroy();
                state.input.devices.remove(&proxy.id());
            }
            Event::Type { _type } => {
                d.keyboard = matches!(_type.into_result(), Ok(Type::Keyboard));
                d.pointer = matches!(_type.into_result(), Ok(Type::Pointer));
            }
            Event::Name { name } => {
                if std::env::var_os("CHAOPHRAYA_DEBUG").is_some() {
                    eprintln!("chaophraya: input device {name:?}");
                }
                d.name = name;
            }
            Event::Done => {
                if d.keyboard {
                    d.proxy.set_repeat_info(rate, delay);
                }
                state.request_libinput_apply();
            }
        }
    }
}

impl Dispatch<RiverXkbConfigV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &RiverXkbConfigV1,
        event: river_xkb_config_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_xkb_config_v1::Event::XkbKeyboard { id } = event {
            if std::env::var_os("CHAOPHRAYA_DEBUG").is_some() {
                eprintln!("chaophraya: xkb keyboard {:?} (keymap ready: {})", id.id(), state.input.keymap_ok);
            }
            if let (true, Some(km)) = (state.input.keymap_ok, &state.input.keymap) {
                id.set_keymap(km);
            }
            state.input.keyboards.push(id);
        }
    }

    wayland_client::event_created_child!(State, RiverXkbConfigV1, [
        river_xkb_config_v1::EVT_XKB_KEYBOARD_OPCODE => (RiverXkbKeyboardV1, ())
    ]);
}

impl Dispatch<RiverXkbKeymapV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverXkbKeymapV1,
        event: river_xkb_keymap_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.input.keymap.as_ref() != Some(proxy) {
            return;
        }
        match event {
            river_xkb_keymap_v1::Event::Success => {
                state.input.keymap_ok = true;
                for k in &state.input.keyboards {
                    k.set_keymap(proxy);
                }
            }
            river_xkb_keymap_v1::Event::Failure { error_msg } => {
                eprintln!("chaophraya: keymap rejected: {error_msg}");
            }
        }
    }
}

impl Dispatch<RiverXkbKeyboardV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverXkbKeyboardV1,
        event: river_xkb_keyboard_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if std::env::var_os("CHAOPHRAYA_DEBUG").is_some() {
            eprintln!("chaophraya: xkb keyboard {:?} event {event:?}", proxy.id());
        }
        match event {
            river_xkb_keyboard_v1::Event::Layout { index, .. } => {
                if state.input.layout_index != Some(index) {
                    state.input.layout_index = Some(index);
                    state.state_changed = true;
                }
            }
            river_xkb_keyboard_v1::Event::Removed => {
                state.input.keyboards.retain(|k| k != proxy);
                proxy.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverLibinputConfigV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &RiverLibinputConfigV1,
        event: river_libinput_config_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_libinput_config_v1::Event::LibinputDevice { id } = event {
            state.input.lidevs.push(LibinputDevice { proxy: id, device: None, tap_fingers: 0, applied: false });
        }
    }

    wayland_client::event_created_child!(State, RiverLibinputConfigV1, [
        river_libinput_config_v1::EVT_LIBINPUT_DEVICE_OPCODE => (RiverLibinputDeviceV1, ())
    ]);
}

impl Dispatch<RiverLibinputDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &RiverLibinputDeviceV1,
        event: river_libinput_device_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use river_libinput_device_v1::Event;
        let Some(l) = state.input.lidevs.iter_mut().find(|l| &l.proxy == proxy) else { return };
        match event {
            Event::InputDevice { device } => {
                l.device = Some(device.id());
                state.request_libinput_apply();
            }
            Event::TapSupport { finger_count } => l.tap_fingers = finger_count,
            Event::Removed => {
                proxy.destroy();
                state.input.lidevs.retain(|l| &l.proxy != proxy);
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverLibinputResultV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &RiverLibinputResultV1,
        event: river_libinput_result_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if !matches!(event, river_libinput_result_v1::Event::Success) {
            eprintln!("chaophraya: input setting not applied: {event:?}");
        }
    }
}
