//! Standard gamepad mappings, hot plugging and radial analog dead zones.
use crate::services::hid;
use gilrs::{Axis, Button, EventType, Gilrs};

pub(super) struct Controllers {
    backend: Option<Gilrs>,
}

impl Controllers {
    pub(super) fn new() -> Self {
        let backend = match Gilrs::new() {
            Ok(pads) => {
                for (_, pad) in pads.gamepads() {
                    eprintln!("controller connected: {}", pad.name());
                }
                Some(pads)
            }
            Err(error) => {
                eprintln!("controller input unavailable: {error}; keyboard remains available");
                None
            }
        };
        Self { backend }
    }

    pub(super) fn poll(&mut self) -> (u32, (i16, i16)) {
        let Some(pads) = self.backend.as_mut() else {
            return (0, (0, 0));
        };
        while let Some(event) = pads.next_event() {
            match event.event {
                EventType::Connected => eprintln!("controller connected: {}", pads.gamepad(event.id).name()),
                EventType::Disconnected => eprintln!("controller disconnected: {}", pads.gamepad(event.id).name()),
                _ => {}
            }
        }
        let Some((_, pad)) = pads.gamepads().find(|(_, pad)| pad.is_connected()) else {
            return (0, (0, 0));
        };
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
