//! Standard gamepad mappings, hot plugging and radial analog dead zones.
use crate::services::hid;
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use std::time::{Duration, Instant};

/// udev enumeration works inside toolbox containers, but host hotplug events
/// need not cross their namespace. Gilrs' inotify backend observes the mounted
/// device directory, including permission changes. Re-exec with a command-specific
/// environment rather than mutating process-global environment around threads.
pub(super) fn configure_discovery() -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("GILRS_DISABLE_UDEV").is_none()
        && ["/run/.containerenv", "/.dockerenv"].iter().any(|p| std::path::Path::new(p).exists())
    {
        use std::os::unix::process::CommandExt;
        eprintln!("controller discovery: watching input-device changes in the container");
        let error = std::process::Command::new(std::env::current_exe()?)
            .args(std::env::args_os().skip(1))
            .env("GILRS_DISABLE_UDEV", "1")
            .exec();
        return Err(error);
    }
    Ok(())
}

struct Selection<T> {
    active: Option<T>,
}

impl<T> Default for Selection<T> {
    fn default() -> Self {
        Self { active: None }
    }
}

impl<T: Copy + PartialEq> Selection<T> {
    fn connected_or_active(&mut self, id: T) {
        self.active = Some(id);
    }
    fn disconnected(&mut self, id: T) {
        if self.active == Some(id) {
            self.active = None;
        }
    }
    fn select(&mut self, available: impl Iterator<Item = T>) -> Option<T> {
        let mut first = None;
        for id in available {
            if self.active == Some(id) {
                return self.active;
            }
            first.get_or_insert(id);
        }
        self.active = first;
        first
    }
}

pub(super) struct Controllers {
    backend: Option<Gilrs>,
    selection: Selection<GamepadId>,
    next_retry: Instant,
    last_error: Option<String>,
}

impl Controllers {
    pub(super) fn new() -> Self {
        let mut this = Self { backend: None, selection: Selection::default(), next_retry: Instant::now(), last_error: None };
        this.open_backend();
        this
    }

    fn open_backend(&mut self) {
        self.next_retry = Instant::now() + Duration::from_secs(2);
        match Gilrs::new() {
            Ok(pads) => {
                if self.last_error.take().is_some() {
                    eprintln!("controller input recovered");
                }
                for (_, pad) in pads.gamepads() {
                    eprintln!("controller connected: {}", pad.name());
                }
                self.backend = Some(pads);
            }
            Err(error) => {
                let error = error.to_string();
                if self.last_error.as_ref() != Some(&error) {
                    eprintln!("controller input unavailable: {error}; retrying while keyboard remains available");
                }
                self.last_error = Some(error);
            }
        }
    }

    pub(super) fn poll(&mut self) -> (u32, (i16, i16)) {
        if self.backend.is_none() && Instant::now() >= self.next_retry {
            self.open_backend();
        }
        let Some(pads) = self.backend.as_mut() else {
            return (0, (0, 0));
        };
        while let Some(event) = pads.next_event() {
            match event.event {
                EventType::Connected => {
                    eprintln!("controller connected: {}", pads.gamepad(event.id).name());
                    self.selection.connected_or_active(event.id);
                }
                EventType::Disconnected => {
                    eprintln!("controller disconnected: {}", pads.gamepad(event.id).name());
                    self.selection.disconnected(event.id);
                }
                EventType::ButtonPressed(..) | EventType::ButtonRepeated(..) => self.selection.connected_or_active(event.id),
                EventType::ButtonChanged(_, value, _) if value > 0.5 => self.selection.connected_or_active(event.id),
                EventType::AxisChanged(_, value, _) if value.abs() > 0.18 => self.selection.connected_or_active(event.id),
                _ => {}
            }
        }
        pads.inc();
        let Some(id) = self.selection.select(pads.gamepads().filter(|(_, p)| p.is_connected()).map(|(id, _)| id)) else {
            return (0, (0, 0));
        };
        let pad = pads.gamepad(id);
        let buttons = [
            (Button::South, 1 << 0),
            (Button::East, 1 << 1),
            (Button::Select, 1 << 2),
            (Button::Start, 1 << 3),
            (Button::DPadRight, 1 << 4),
            (Button::DPadLeft, 1 << 5),
            (Button::DPadUp, 1 << 6),
            (Button::DPadDown, 1 << 7),
            (Button::RightTrigger, 1 << 8),
            (Button::LeftTrigger, 1 << 9),
            (Button::North, 1 << 10),
            (Button::West, 1 << 11),
        ];
        let mut held = buttons.into_iter().filter(|(button, _)| pad.is_pressed(*button)).fold(0, |bits, (_, bit)| bits | bit);
        let stick = analog(pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
        for (value, positive, negative) in [(stick.0, 28, 29), (stick.1, 30, 31)] {
            if value > 40 {
                held |= 1 << positive;
            }
            if value < -40 {
                held |= 1 << negative;
            }
        }
        (held, stick)
    }
}

fn analog(x: f32, y: f32) -> (i16, i16) {
    if !x.is_finite() || !y.is_finite() {
        return (0, 0);
    }
    let length = x.hypot(y);
    const DEAD_ZONE: f32 = 0.18;
    if length <= DEAD_ZONE {
        return (0, 0);
    }
    let magnitude = (length.min(1.0) - DEAD_ZONE) / (1.0 - DEAD_ZONE);
    let factor = magnitude / length * f32::from(hid::CIRCLE_PAD_MAX);
    ((x * factor).round() as i16, (y * factor).round() as i16)
}

/// Keyboard direction keys take precedence while held; releasing a keyboard
/// button cannot cancel a button or stick still held on the controller.
pub(super) fn combine(keyboard: u32, controller: (u32, (i16, i16))) -> u64 {
    let circle = if keyboard & (0xF << 28) != 0 { hid::circle_pad(keyboard) } else { controller.1 };
    hid::pack_live(keyboard | controller.0, circle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_connection_replaces_idle_receiver_and_reconnects_without_stale_selection() {
        let mut selection = Selection::default();
        assert_eq!(selection.select([].into_iter()), None);
        assert_eq!(selection.select([0].into_iter()), Some(0));
        selection.connected_or_active(1);
        assert_eq!(selection.select([0, 1].into_iter()), Some(1));
        selection.disconnected(1);
        assert_eq!(selection.select([0].into_iter()), Some(0));
        selection.connected_or_active(2);
        assert_eq!(selection.select([0, 2].into_iter()), Some(2));
        selection.connected_or_active(0);
        assert_eq!(selection.select([0, 2].into_iter()), Some(0));
        assert_eq!(selection.select([].into_iter()), None);
    }
    #[test]
    fn dead_zone_rescales_and_caps_the_radial_stick() {
        assert_eq!(analog(0.1, -0.1), (0, 0));
        assert_eq!(analog(1.0, 0.0), (156, 0));
        assert_eq!(analog(-1.0, 0.0), (-156, 0));
        let (x, y) = analog(1.0, 1.0);
        assert!((f32::from(x).hypot(f32::from(y)) - 156.0).abs() < 1.0);
        assert_eq!(analog(f32::NAN, 0.5), (0, 0));
        let slow = analog(0.3, 0.0).0;
        assert!((1..40).contains(&slow));
    }
    #[test]
    fn keyboard_and_controller_inputs_do_not_release_each_other() {
        let controller = (1, (34, -80));
        assert_eq!(hid::unpack_live(combine(1 << 28, controller)), (1 | 1 << 28, (156, 0)));
        assert_eq!(hid::unpack_live(combine(0, controller)), controller);
        assert_eq!(hid::unpack_live(combine(1, (0, (0, 0)))), (1, (0, 0)));
    }
}
