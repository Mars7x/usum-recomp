//! `dsp::DSP`: the audio DSP. Audio is to be high-level emulated: the
//! DSP firmware the game loads is accepted and identified, not run (a
//! DSP core is out of scope). Commands per 3dbrew "DSP Services", added
//! when the game first sends them.

use std::collections::VecDeque;

use super::Message;

/// The audio pipe and the DSP shared memory (3dbrew "DSP Memory Region"):
/// two regions of 0x8000 bytes at ARM 0x1FF50000 and 0x1FF70000, used as
/// a double buffer, each holding 15 structures whose DSP addresses the
/// firmware reports on pipe 2 after the application initializes it.
///
/// The addresses are the firmware's choice and 3dbrew does not list them
/// ("may vary depending on the exact DSP firmware"); the game reads them
/// from the pipe. So they are ours: the structures in 3dbrew's order from
/// the start of the region, with 3dbrew's sizes where it gives them and
/// 0x800 bytes where it does not (a choice, R-028).
pub const STRUCTURE_SIZES: [u32; 15] = [
    0x10,     // frame count
    192 * 24, // input (source) configurations
    12 * 24,  // input statuses
    32 * 24,  // input ADPCM coefficients
    0x800,    // DSP configuration
    32,       // DSP status
    640,      // output samples
    5120,     // intermediate mix samples
    0x800,    // compressor table
    0x800,    // debug statistics
    0x800,    // unknown coefficients
    0x800,    // unknown coefficients
    0x800,    // unknown coefficients
    0x800,    // surround biquad filter 1
    0x800,    // surround biquad filter 2
];

/// Region 0 in DSP data memory, in 16-bit words: ARM address
/// (word << 1) + 0x1FF40000 (3dbrew "DSP:ConvertProcessAddressFromDspDram",
/// "typically"); region 1 is `word | 0x10000` (the game's own code at
/// 0x0025E780 forms it that way).
pub const REGION0_WORD: u32 = 0x8000;
pub const REGION_BYTES: u32 = 0x8000;

pub fn arm_address(dsp_word: u32) -> u32 {
    (dsp_word << 1).wrapping_add(0x1FF4_0000)
}

/// The DSP word addresses of the structures in region 0.
pub fn structure_words() -> [u16; 15] {
    let mut out = [0u16; 15];
    let mut offset = 0;
    for (word, size) in out.iter_mut().zip(STRUCTURE_SIZES) {
        *word = (REGION0_WORD + offset / 2) as u16;
        offset += size;
    }
    out
}

/// Pipe 2 is "audio" (3dbrew "DSP:ReadPipeIfPossible").
pub const AUDIO_PIPE: u32 = 2;
/// The DSP interrupt for pipes (3dbrew "DSP:RegisterInterruptEvents").
pub const PIPE_INTERRUPT: u32 = 2;

/// What the HLE DSP has to say back to the application, per pipe.
#[derive(Debug, Default)]
pub struct Pipes {
    from_dsp: [VecDeque<u8>; 8],
    running: bool,
    shutdown_ack: Option<u16>,
}

impl Pipes {
    /// The application wrote `bytes` to `channel`. Returns true if the
    /// DSP answers (data is now readable, so the pipe's event fires).
    ///
    /// Audio pipe u16 state (libctru NDSP protocol): 0 initialize / 2 resume
    /// return the structure count and addresses; 1 shutdown / 3 suspend stop
    /// processing and acknowledge completion through receive register zero.
    pub fn write(&mut self, channel: u32, bytes: &[u8]) -> Result<bool, String> {
        match (channel, bytes) {
            (AUDIO_PIPE, [0 | 2, 0, ..]) => {
                self.running = true;
                self.shutdown_ack = None;
                let words = structure_words();
                let pipe = &mut self.from_dsp[AUDIO_PIPE as usize];
                pipe.clear();
                pipe.extend((words.len() as u16).to_le_bytes());
                pipe.extend(words.iter().flat_map(|w| w.to_le_bytes()));
                Ok(true)
            }
            // libctru ndspFinalize: 1 shuts down, 3 suspends; register 0
            // returns 1 when complete. The upper two bytes are not command data.
            (AUDIO_PIPE, [1 | 3, 0, ..]) => {
                self.running = false;
                self.shutdown_ack = Some(1);
                self.from_dsp[AUDIO_PIPE as usize].clear();
                Ok(false)
            }
            _ => Err(format!("DSP pipe {channel} write {bytes:02x?}")),
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn recv_ready(&self, register: u32) -> Result<bool, String> {
        if register != 0 {
            return Err(format!("DSP receive register {register}"));
        }
        Ok(self.shutdown_ack.is_some())
    }

    pub fn recv(&mut self, register: u32) -> Result<u16, String> {
        if register != 0 {
            return Err(format!("DSP receive register {register}"));
        }
        self.shutdown_ack.take().ok_or_else(|| "DSP receive register 0 is empty".into())
    }

    pub fn unload(&mut self) {
        *self = Self::default();
    }

    /// Up to `size` bytes the DSP has written to `channel`.
    pub fn read(&mut self, channel: u32, size: u32) -> Vec<u8> {
        let pipe = &mut self.from_dsp[channel as usize & 7];
        let n = (size as usize).min(pipe.len());
        pipe.drain(..n).collect()
    }
}

pub enum Request {
    /// LoadComponent (0x0011, 3dbrew "DSP:LoadComponent"): `size` bytes of
    /// firmware at `buffer`.
    LoadComponent {
        size: u32,
        buffer: u32,
    },
    /// RegisterInterruptEvents (0x0015, 3dbrew): `event` is signaled on
    /// DSP `interrupt` for `channel`; handle 0 unregisters.
    RegisterInterruptEvents {
        interrupt: u32,
        channel: u32,
        event: u32,
    },
    /// WriteProcessPipe (0x000D): `size` bytes at `buffer` to pipe
    /// `channel` (a static buffer, descriptor index 1).
    WriteProcessPipe {
        channel: u32,
        size: u32,
        buffer: u32,
    },
    /// ReadPipeIfPossible (0x0010): up to `size` bytes of pipe `channel`
    /// from `peer` (0: the DSP) into the static buffer at `buffer`.
    ReadPipeIfPossible {
        channel: u32,
        peer: u32,
        size: u32,
        buffer: u32,
    },
    /// ConvertProcessAddressFromDspDram (0x000C).
    ConvertProcessAddressFromDspDram(u32),
    Other(u32),
}

pub fn parse(message: &Message) -> Request {
    let w = &message.words;
    match w[0] {
        0x0011_00C2 if w[4] == (w[1] << 4) | 0xA => Request::LoadComponent { size: w[1], buffer: w[5] },
        0x0015_0082 if w[3] == super::srv::COPY_HANDLE => {
            Request::RegisterInterruptEvents { interrupt: w[1], channel: w[2], event: w[4] }
        }
        0x000D_0082 if w[3] == (w[2] << 14) | (1 << 10) | 2 => {
            Request::WriteProcessPipe { channel: w[1], size: w[2], buffer: w[4] }
        }
        0x0010_00C0 => Request::ReadPipeIfPossible { channel: w[1], peer: w[2], size: w[3] & 0xFFFF, buffer: message.statics[1] },
        0x000C_0040 => Request::ConvertProcessAddressFromDspDram(w[1]),
        other => Request::Other(other),
    }
}

/// Header 0x00110082: result, u8 loaded, the buffer descriptor and address.
pub fn load_component_reply(size: u32, buffer: u32) -> Vec<u32> {
    vec![super::header(0x11, 2, 2), 0, 1, (size << 4) | 0xA, buffer]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_shutdown_suspend_and_resume_use_mailbox_acknowledgements() {
        let mut pipes = Pipes::default();
        assert!(!pipes.running());
        assert!(!pipes.recv_ready(0).unwrap());
        pipes.write(AUDIO_PIPE, &[0, 0, 0x6b, 0]).unwrap();
        assert!(pipes.running());
        let init = pipes.read(AUDIO_PIPE, 4096);
        assert_eq!(init.len(), 32);
        for command in [1, 3] {
            assert!(!pipes.write(AUDIO_PIPE, &[command, 0, 0x6b, 0]).unwrap());
            assert!(!pipes.running());
            assert!(pipes.recv_ready(0).unwrap());
            assert_eq!(pipes.recv(0).unwrap(), 1);
            assert!(!pipes.recv_ready(0).unwrap());
            assert!(pipes.recv(0).is_err());
            assert!(pipes.read(AUDIO_PIPE, 4096).is_empty());
            pipes.write(AUDIO_PIPE, &[2, 0, 0, 0]).unwrap();
            assert!(pipes.running());
            assert_eq!(pipes.read(AUDIO_PIPE, 4096), init);
        }
        assert!(pipes.recv_ready(3).is_err());
        pipes.unload();
        assert!(!pipes.running());
        assert!(!pipes.recv_ready(0).unwrap());
    }

    #[test]
    fn the_structures_fit_one_region_in_order() {
        let words = structure_words();
        assert_eq!(arm_address(u32::from(words[0])), 0x1FF5_0000);
        assert!(words.windows(2).all(|w| w[0] < w[1]));
        let end = STRUCTURE_SIZES.iter().sum::<u32>();
        assert!(end <= REGION_BYTES, "{end:#x}");
        assert_eq!(arm_address(u32::from(words[14]) | 0x10000) & !0x7FFF, 0x1FF7_0000, "region 1");
    }

    #[test]
    fn initialize_answers_with_count_and_addresses() {
        let mut p = Pipes::default();
        assert_eq!(p.write(AUDIO_PIPE, &[0, 0, 0, 0]), Ok(true));
        assert_eq!(p.read(AUDIO_PIPE, 2), [15, 0]);
        let rest = p.read(AUDIO_PIPE, 0x100);
        assert_eq!(rest.len(), 30);
        assert_eq!(&rest[..2], &0x8000u16.to_le_bytes());
        assert!(p.read(AUDIO_PIPE, 2).is_empty());
    }
}
