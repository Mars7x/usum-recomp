//! `cfg:u`: the console's settings, read as config blocks (3dbrew
//! "Config Savegame" for the block contents, "Config Services" for the
//! commands). The emulated console's settings are `Console`: explicit
//! values, chosen by the runtime or the user, never read from a real
//! console. Blocks are added when the game first asks for them.

use super::Message;

/// Language IDs (3dbrew block 0x000A0002).
pub const LANGUAGE_ENGLISH: u8 = 1;

/// Block 0x00050005, stereo display settings: eight floats that 3dbrew
/// says are hard-coded in the cfg module (pupillary distance 62 mm, eye
/// to upper screen 289 mm, Old 3DS upper screen 76.80 × 46.08 mm, then
/// four values 3dbrew lists without meanings).
pub const STEREO_DISPLAY: [f32; 8] = [62.0, 289.0, 76.80, 46.08, 10.0, 5.0, 55.58, 21.57];

/// The settings of the emulated console.
#[derive(Debug, Clone)]
pub struct Console {
    /// SecureInfoGetRegion: the console's region, which must be the
    /// cartridge's (a USA game on a USA console).
    pub region: u8,
    /// Block 0x000A0002. Default English (the USA cartridges support
    /// English, Japanese, French, German, Spanish, Italian, Chinese and
    /// Korean); meant to become a user setting.
    pub language: u8,
    /// Block 0x00070001: 0 mono, 1 stereo, 2 surround. Stereo (a choice:
    /// the host plays two channels).
    pub sound_output: u8,
    /// The console-unique value behind GenHashConsoleUnique (3dbrew:
    /// config block 0x90001, generated from the console's FriendCodeSeed).
    /// There is no real console: a fixed value, a choice. It should
    /// become per installation and kept with the user's settings.
    pub console_unique: u64,
    /// Block 0x000B0000 bytes 2 and 3: state/province and country (the
    /// DSi/Wii codes; 49 is the United States). Bytes 0 and 1 are unknown
    /// to 3dbrew and 0 here; the state is 0 too (a choice).
    pub country: u8,
    pub province: u8,
    /// Block 0x000A0000: at most 10 UTF-16 units. "Player" (a choice;
    /// meant to become a user setting).
    pub user_name: String,
    /// Block 0x000A0001: month, day. 1 January (a choice).
    pub birthday: (u8, u8),
}

/// `text` as UTF-16LE in a zeroed slot of `size` bytes.
fn utf16(text: &str, size: usize) -> Vec<u8> {
    let mut b: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    b.resize(size, 0);
    b
}

impl Default for Console {
    fn default() -> Console {
        Console {
            region: usum_import::profile::Region::Usa as u8,
            language: LANGUAGE_ENGLISH,
            sound_output: 1,
            console_unique: 0,
            country: 49,
            province: 0,
            user_name: "Player".into(),
            birthday: (1, 1),
        }
    }
}

impl Console {
    /// The console a recorded release expects.
    pub fn for_region(region: usum_import::profile::Region) -> Console {
        Console { region: region as u8, ..Console::default() }
    }

    /// The contents of block `id`, if this console defines it.
    pub fn block(&self, id: u32) -> Option<Vec<u8>> {
        match id {
            0x000A_0002 => Some(vec![self.language]),
            0x0007_0001 => Some(vec![self.sound_output]),
            // user time offset (displayed minus RTC): 0, the RTC being
            // the host's clock
            0x0003_0001 => Some(vec![0; 8]),
            // parental controls off (bitmask bit 0 clear; maximum age 20,
            // "no restriction"), COPPACS data none (3dbrew gives no
            // layout): choices, an unrestricted console
            0x000C_0000 => {
                let mut b = vec![0; 0xC0];
                b[9] = 20;
                Some(b)
            }
            0x000C_0001 => Some(vec![0; 0x14]),
            // country name per language (12 used slots of 0x80 bytes, 16
            // in all): the English name in each (not translated, a
            // choice); the state name: none, as no state is set
            0x000B_0001 if self.country == 49 => {
                Some((0..16).flat_map(|k| utf16(if k < 12 { "United States" } else { "" }, 0x80)).collect())
            }
            0x000B_0002 => Some(vec![0; 0x800]),
            // coordinates (latitude, longitude as s16 × 180/32768): 0, 0
            // as no state is set (a choice)
            0x000B_0003 => Some(vec![0; 4]),
            0x000A_0000 => {
                let mut b = utf16(&self.user_name, 0x16);
                b.truncate(0x16);
                b.resize(0x1C, 0);
                Some(b)
            }
            0x000A_0001 => Some(vec![self.birthday.0, self.birthday.1]),
            // the current local friend code (3dbrew "Config Savegame"):
            // 0, as no friends account exists (a choice, matching
            // frd's GetMyFriendKey)
            0x0009_0000 => Some(vec![0; 8]),
            0x000B_0000 => Some(vec![0, 0, self.province, self.country]),
            0x0005_0005 => Some(STEREO_DISPLAY.iter().flat_map(|f| f.to_le_bytes()).collect()),
            _ => None,
        }
    }
}

impl Console {
    /// 3dbrew "Cfg:GetRegionCanadaUSA": region 1 (America) and country
    /// 18 (Canada) or 49 (USA).
    pub fn region_canada_usa(&self) -> bool {
        self.region == 1 && matches!(self.country, 18 | 49)
    }

    /// 3dbrew "Cfg:GenHashConsoleUnique": the last two words of SHA-256
    /// over the console-unique u64, then the u32 salt.
    pub fn hash_console_unique(&self, salt: u32) -> (u32, u32) {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.console_unique.to_le_bytes());
        h.update(salt.to_le_bytes());
        let d = h.finalize();
        (u32::from_le_bytes(d[24..28].try_into().expect("4")), u32::from_le_bytes(d[28..32].try_into().expect("4")))
    }
}

pub enum Request {
    /// GetConfigInfoBlk2 (0x0001): `size` bytes of block `id` into the
    /// mapped buffer at `buffer`.
    GetConfigInfoBlk2 {
        size: u32,
        id: u32,
        buffer: u32,
    },
    /// SecureInfoGetRegion (0x0002): result, u8 region.
    SecureInfoGetRegion,
    /// GetRegionCanadaUSA (0x0004): result, u8.
    GetRegionCanadaUsa,
    /// GenHashConsoleUnique (0x0003): a 20-bit app ID salt.
    GenHashConsoleUnique(u32),
    Other(u32),
}

pub fn parse(message: &Message) -> Request {
    let w = &message.words;
    match w[0] {
        0x0002_0000 => Request::SecureInfoGetRegion,
        0x0004_0000 => Request::GetRegionCanadaUsa,
        0x0003_0040 => Request::GenHashConsoleUnique(w[1] & 0xF_FFFF),
        0x0001_0082 if w[3] == (w[1] << 4) | 0xC => Request::GetConfigInfoBlk2 { size: w[1], id: w[2], buffer: w[4] },
        other => Request::Other(other),
    }
}

/// Result, then the buffer descriptor and address given back.
pub fn block_reply(command: u16, size: u32, buffer: u32) -> Vec<u32> {
    vec![super::header(command, 1, 2), 0, (size << 4) | 0xC, buffer]
}
