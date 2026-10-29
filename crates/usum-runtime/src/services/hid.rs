//! `hid:USER` / `hid:SPVR`: buttons, circle pad, touch screen, motion
//! (3dbrew "HID Services"). Input state lives in shared memory (3dbrew
//! "HID Shared Memory", 0x2B0 bytes), written each VBlank from the
//! input script.

/// 0x2B0 bytes rounded up to the page mappings are made of.
pub const SHARED_MEMORY_SIZE: u32 = 0x1000;

/// 3dbrew "HID:GetIPCHandles": result, a descriptor copying 6 handles
/// (0x14000000), the shared memory, then the events for PAD/touch (twice),
/// accelerometer, gyroscope, DebugPad.
pub fn ipc_handles_reply(memory: u32, events: [u32; 5]) -> Vec<u32> {
    let mut w = vec![super::header(0xA, 1, 7), 0, super::apt::copy_handles(6), memory];
    w.extend(events);
    w
}

/// Host motion input is not wired yet. Enabled sensors publish neutral samples
/// and normal ring timestamps/events rather than leaving a stale shared page.
#[derive(Clone, Debug, Default)]
pub struct Motion {
    enabled: [bool; 2],
    initialized: [bool; 2],
}

impl Motion {
    /// libctru HIDUSER command IDs: enable/disable accelerometer, then gyro.
    pub fn request(&mut self, header: u32) -> Option<Vec<u32>> {
        let (sensor, enabled) = match header {
            0x0011_0000 => (0, true),
            0x0012_0000 => (0, false),
            0x0013_0000 => (1, true),
            0x0014_0000 => (1, false),
            _ => return None,
        };
        if self.enabled[sensor] != enabled {
            self.initialized[sensor] = false;
        }
        self.enabled[sensor] = enabled;
        Some(super::result_reply((header >> 16) as u16))
    }

    pub fn enabled(&self, sensor: usize) -> bool {
        self.enabled[sensor]
    }

    /// 3dbrew HID shared memory: six-byte vectors, 8 accel / 32 gyro entries.
    /// The two timestamps describe consecutive updates of entry zero.
    pub fn update(&mut self, mem: &mut usum_guest::Memory, base: u32, ticks: u64) -> Result<(), usum_guest::Fault> {
        for (sensor, (offset, entries)) in [(0x108, 8), (0x158, 32)].into_iter().enumerate() {
            if !self.enabled[sensor] {
                continue;
            }
            let at = base + offset;
            let index = if self.initialized[sensor] { mem.read_u32(at + 0x10)?.wrapping_add(1) % entries } else { 0 };
            if index == 0 {
                let old = mem.read_range(at, 8)?;
                mem.device_write(at + 8, &old)?;
                mem.device_write(at, &ticks.to_le_bytes())?;
            }
            mem.device_write(at + 0x18, &[0; 6])?;
            mem.device_write(at + 0x20 + 6 * index, &[0; 6])?;
            mem.device_write(at + 0x10, &index.to_le_bytes())?;
            self.initialized[sensor] = true;
        }
        Ok(())
    }
}

/// PAD bits (3dbrew "HID Shared Memory"). The circle pad directions
/// (bits 28-31) also set the stick position (`circle_pad`).
pub const BUTTONS: [(&str, u32); 16] = [
    ("A", 1 << 0),
    ("B", 1 << 1),
    ("SELECT", 1 << 2),
    ("START", 1 << 3),
    ("RIGHT", 1 << 4),
    ("LEFT", 1 << 5),
    ("UP", 1 << 6),
    ("DOWN", 1 << 7),
    ("R", 1 << 8),
    ("L", 1 << 9),
    ("X", 1 << 10),
    ("Y", 1 << 11),
    ("CRIGHT", 1 << 28),
    ("CLEFT", 1 << 29),
    ("CUP", 1 << 30),
    ("CDOWN", 1 << 31),
];

/// Full deflection of the circle pad, in the units HID reports. 3dbrew
/// does not give the range; about ±156 is the usual figure for the
/// calibrated values (a choice until measured).
pub const CIRCLE_PAD_MAX: i16 = 156;

/// The circle pad (x, y; up and right positive) the direction bits in
/// `pad` describe.
pub fn circle_pad(pad: u32) -> (i16, i16) {
    let axis = |plus: u32, minus: u32| match (pad & plus != 0, pad & minus != 0) {
        (true, false) => CIRCLE_PAD_MAX,
        (false, true) => -CIRCLE_PAD_MAX,
        _ => 0,
    };
    (axis(1 << 28, 1 << 29), axis(1 << 30, 1 << 31))
}

/// A scripted input: from VBlank `at` on, these buttons are held and the
/// touch screen is touched at this point (x, y in pixels), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub at: u64,
    pub pad: u32,
    /// Explicit analog position; None preserves the legacy direction-bit input.
    pub circle: Option<(i16, i16)>,
    pub touch: Option<(u16, u16)>,
}

/// The bottom screen's size: touch coordinates are pixels on it.
pub const TOUCH_SIZE: (u16, u16) = (320, 240);

/// Parses "120=A+UP,126=,200=T160:120" (VBlank number = buttons joined by
/// '+', and `Tx:y` for a touch; empty for none). Steps must be in order.
pub fn parse_script(text: &str) -> Result<Vec<Step>, String> {
    let mut steps: Vec<Step> = Vec::new();
    for item in text.split(',').filter(|i| !i.trim().is_empty()) {
        let (at, buttons) = item.split_once('=').ok_or_else(|| format!("input step {item:?}: expected VBLANK=BUTTONS"))?;
        let at: u64 = at.trim().parse().map_err(|_| format!("input step {item:?}: bad VBlank number"))?;
        let (mut pad, mut touch, mut circle) = (0, None, None);
        for name in buttons.split('+').map(str::trim).filter(|n| !n.is_empty()) {
            if let Some((x, y)) = name.strip_prefix(['T', 't']).and_then(|xy| xy.split_once(':')) {
                let (x, y) = (x.parse::<u16>(), y.parse::<u16>());
                match (x, y) {
                    (Ok(x), Ok(y)) if x < TOUCH_SIZE.0 && y < TOUCH_SIZE.1 => touch = Some((x, y)),
                    _ => return Err(format!("input step {item:?}: touch outside 0..320 x 0..240")),
                }
                continue;
            }
            if let Some((x, y)) = name.strip_prefix(['C', 'c']).and_then(|xy| xy.split_once(':')) {
                match (x.parse::<i16>(), y.parse::<i16>()) {
                    (Ok(x), Ok(y))
                        if (-CIRCLE_PAD_MAX..=CIRCLE_PAD_MAX).contains(&x) && (-CIRCLE_PAD_MAX..=CIRCLE_PAD_MAX).contains(&y) =>
                    {
                        circle = Some((x, y))
                    }
                    _ => return Err(format!("input step {item:?}: circle pad outside -156..156")),
                }
                continue;
            }
            let bit =
                BUTTONS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).ok_or_else(|| format!("unknown button {name:?}"))?.1;
            pad |= bit;
        }
        if steps.last().is_some_and(|s| s.at > at) {
            return Err("input steps must be in VBlank order".into());
        }
        steps.push(Step { at, pad, touch, circle });
    }
    Ok(steps)
}

/// A script `parse_script` reads back: one step per change.
pub fn format_script(steps: &[Step]) -> String {
    let step = |s: &Step| {
        let mut parts: Vec<String> =
            BUTTONS.iter().filter(|(_, bit)| s.pad & bit != 0).map(|(name, _)| (*name).to_owned()).collect();
        if let Some((x, y)) = s.circle {
            parts.push(format!("C{x}:{y}"));
        }
        if let Some((x, y)) = s.touch {
            parts.push(format!("T{x}:{y}"));
        }
        format!("{}={}", s.at, parts.join("+"))
    };
    steps.iter().map(step).collect::<Vec<_>>().join(",")
}

/// The buttons held at VBlank `n`.
pub fn held(script: &[Step], n: u64) -> u32 {
    script.iter().take_while(|s| s.at <= n).last().map_or(0, |s| s.pad)
}

/// Analog input at VBlank `n`, with old recordings retaining digital sticks.
pub fn circle_at(script: &[Step], n: u64) -> (i16, i16) {
    script.iter().take_while(|s| s.at <= n).last().map_or((0, 0), |s| s.circle.unwrap_or_else(|| circle_pad(s.pad)))
}

/// One coherent live snapshot: buttons in low bits, signed stick axes above.
pub fn pack_live(pad: u32, (x, y): (i16, i16)) -> u64 {
    u64::from(pad) | u64::from(x as u16) << 32 | u64::from(y as u16) << 48
}
pub fn unpack_live(value: u64) -> (u32, (i16, i16)) {
    (value as u32, ((value >> 32) as i16, (value >> 48) as i16))
}

/// Where the touch screen is touched at VBlank `n`, if it is.
pub fn touched(script: &[Step], n: u64) -> Option<(u16, u16)> {
    script.iter().take_while(|s| s.at <= n).last().and_then(|s| s.touch)
}

/// A touch packed into one u32 for a frontend to share: bit 31 set when
/// touched, x in bits 0-15, y in bits 16-30.
pub fn pack_touch(touch: Option<(u16, u16)>) -> u32 {
    touch.map_or(0, |(x, y)| 1 << 31 | u32::from(y & 0x7FFF) << 16 | u32::from(x))
}

pub fn unpack_touch(packed: u32) -> Option<(u16, u16)> {
    (packed >> 31 == 1).then_some((packed as u16, (packed >> 16) as u16 & 0x7FFF))
}

#[cfg(test)]
mod tests {

    #[test]
    fn motion_requests_publish_bounded_rings_and_stop_when_disabled() {
        use super::Motion;
        use usum_guest::{memory::Perm, Memory};
        let mut motion = Motion::default();
        let mut mem = Memory::new();
        let base = 0x1000;
        mem.map(base, vec![0; 4096], Perm::R).unwrap();
        mem.device_write(base + 0x238, &[0x93; 8]).unwrap();
        assert!(motion.request(0x0011_0040).is_none());
        motion.update(&mut mem, base, 1).unwrap();
        assert_eq!(mem.read_u32(base + 0x108).unwrap(), 0);
        for (enable, disable, offset, count, sensor) in
            [(0x0011_0000, 0x0012_0000, 0x108, 8, 0), (0x0013_0000, 0x0014_0000, 0x158, 32, 1)]
        {
            assert_eq!(motion.request(enable).unwrap(), super::super::result_reply((enable >> 16) as u16));
            assert!(motion.enabled(sensor));
            for step in 0..=count {
                motion.update(&mut mem, base, 10 + u64::from(step)).unwrap();
                assert_eq!(mem.read_u32(base + offset + 0x10).unwrap(), step % count);
            }
            assert_eq!(mem.read_u32(base + offset).unwrap(), 10 + count);
            assert_eq!(mem.read_u32(base + offset + 8).unwrap(), 10);
            assert_eq!(mem.read_range(base + offset + 0x18, 6).unwrap(), vec![0; 6]);
            let before = mem.read_range(base + offset, 0x20 + 6 * count).unwrap();
            motion.request(disable).unwrap();
            motion.update(&mut mem, base, 100).unwrap();
            assert!(!motion.enabled(sensor));
            assert_eq!(mem.read_range(base + offset, 0x20 + 6 * count).unwrap(), before);
            motion.request(enable).unwrap();
            motion.update(&mut mem, base, 200).unwrap();
            assert_eq!(mem.read_u32(base + offset + 0x10).unwrap(), 0);
            motion.request(disable).unwrap();
        }
        assert_eq!(mem.read_range(base + 0x238, 8).unwrap(), vec![0x93; 8]);
    }
    use super::*;

    #[test]
    fn analog_recording_round_trips_and_releases() {
        let steps = parse_script("1=A+C34:-80,2=C-156:156+T3:4,3=").unwrap();
        assert_eq!(circle_at(&steps, 0), (0, 0));
        assert_eq!(circle_at(&steps, 1), (34, -80));
        assert_eq!(circle_at(&steps, 2), (-156, 156));
        assert_eq!(circle_at(&steps, 3), (0, 0));
        assert_eq!(parse_script(&format_script(&steps)).unwrap(), steps);
        for invalid in ["1=C157:0", "1=C-32768:1", "1=C0:99999"] {
            assert!(parse_script(invalid).is_err());
        }
        for circle in [(0, 0), (-156, 156), (34, -80)] {
            assert_eq!(unpack_live(pack_live(0xF000_0FFF, circle)), (0xF000_0FFF, circle));
        }
    }

    #[test]
    fn scripts() {
        let s = parse_script("120=A,126=,200=up+b").unwrap();
        assert_eq!((held(&s, 0), held(&s, 121), held(&s, 130), held(&s, 300)), (0, 1, 0, (1 << 6) | (1 << 1)));
        assert!(parse_script("5=Z").is_err() && parse_script("9=A,3=B").is_err());
        let s = parse_script("1=T10:20+a,5=").unwrap();
        assert_eq!((touched(&s, 1), held(&s, 1), touched(&s, 5)), (Some((10, 20)), 1, None));
        assert!(parse_script("1=T320:0").is_err());
        assert_eq!(unpack_touch(pack_touch(Some((319, 239)))), Some((319, 239)));
        assert_eq!(unpack_touch(pack_touch(None)), None);
        let steps = parse_script("5=A+CUP+T3:4,9=,12=START").unwrap();
        assert_eq!(parse_script(&format_script(&steps)).unwrap(), steps, "round trip");
        let s = parse_script("1=cup+cright").unwrap();
        assert_eq!(circle_pad(held(&s, 1)), (156, 156));
        assert_eq!(circle_pad(1 << 31), (0, -156));
    }
}
