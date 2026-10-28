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
        let (mut pad, mut touch) = (0, None);
        for name in buttons.split('+').map(str::trim).filter(|n| !n.is_empty()) {
            if let Some((x, y)) = name.strip_prefix(['T', 't']).and_then(|xy| xy.split_once(':')) {
                let (x, y) = (x.parse::<u16>(), y.parse::<u16>());
                match (x, y) {
                    (Ok(x), Ok(y)) if x < TOUCH_SIZE.0 && y < TOUCH_SIZE.1 => touch = Some((x, y)),
                    _ => return Err(format!("input step {item:?}: touch outside 0..320 x 0..240")),
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
        steps.push(Step { at, pad, touch });
    }
    Ok(steps)
}

/// A script `parse_script` reads back: one step per change.
pub fn format_script(steps: &[Step]) -> String {
    let step = |s: &Step| {
        let mut parts: Vec<String> =
            BUTTONS.iter().filter(|(_, bit)| s.pad & bit != 0).map(|(name, _)| (*name).to_owned()).collect();
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
    use super::*;

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
