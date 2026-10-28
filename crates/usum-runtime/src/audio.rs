//! The DSP's per-frame work, high-level: each audio frame (160 samples)
//! it reads the application's source configurations from the current
//! shared memory region, advances every source through its buffers, and
//! writes the source statuses into the other region (3dbrew "DSP Memory
//! Region"). Each source's buffers are decoded (PCM8, PCM16, DSP-ADPCM),
//! played at the source's rate with linear interpolation, and mixed by
//! the source's gains for intermediate mix 0 into 160 stereo samples per
//! frame (`Dsp::output`). Mixes 1 and 2 feed effects (reverb and the
//! like), which are not emulated; they are left out rather than added dry
//! (a choice). Filters and the master volume are not applied yet.
//!
//! Facts measured from Ultra Sun's own writes (R-035):
//! - The application increments both regions' frame counters (region 0
//!   even, region 1 odd); the higher is current.
//! - Addresses, sample counts and positions are stored as two halfwords,
//!   high first. Floats and the dirty flags are plain little-endian.
//! - The application only sets dirty flags; the DSP clears them.
//!
//! The dirty flag bits, and that statuses go to the region that is not
//! current, follow Citra's HLE (read as documentation, R-035). The bits
//! named here agree with every flag set Ultra Sun writes.

use usum_guest::memory::Memory;

use crate::services::dsp;

pub const SOURCES: usize = 24;
pub const SAMPLES_PER_FRAME: f64 = 160.0;

/// The two regions' ARM addresses.
pub const REGIONS: [u32; 2] = [0x1FF5_0000, 0x1FF7_0000];

const CONFIG_SIZE: u32 = 192;
const STATUS_SIZE: u32 = 12;

/// Source configuration dirty flags (plain little-endian u32 at +0).
/// Mixed samples shared with a consumer (the play window's sound).
pub type Queue = std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<[i16; 2]>>>;

/// The DSP's output rate: 160 samples per audio frame of 4,888,780 ns.
pub const OUTPUT_RATE: u32 = 32728;

pub mod dirty {
    pub const FORMAT: u32 = 1 << 0;
    pub const MONO_OR_STEREO: u32 = 1 << 1;
    pub const ADPCM_COEFFICIENTS: u32 = 1 << 2;
    pub const ENABLE: u32 = 1 << 16;
    pub const RATE: u32 = 1 << 18;
    pub const BUFFER_QUEUE: u32 = 1 << 19;
    pub const PLAY_POSITION: u32 = 1 << 21;
    pub const FILTERS_ENABLED: u32 = 1 << 22;
    pub const SIMPLE_FILTER: u32 = 1 << 23;
    pub const BIQUAD_FILTER: u32 = 1 << 24;
    pub const SYNC: u32 = 1 << 28;
    pub const RESET: u32 = 1 << 29;
    pub const EMBEDDED_BUFFER: u32 = 1 << 30;
}

/// A buffer queued for a source: its ID, length in samples, and whether
/// it loops; where its samples are (physical address) and, for ADPCM, the
/// decoder context to start from if the application gave one.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Buffer {
    id: u16,
    samples: u32,
    looping: bool,
    /// The sample to start at (the embedded buffer's play position).
    start: u32,
    address: u32,
    adpcm: Option<(i16, i16)>,
}

/// Format bits (3dbrew): channels in bits 0-1 (2: stereo), the codec in
/// bits 2-3 (0 PCM8, 1 PCM16, 2 ADPCM).
fn stereo(format: u16) -> bool {
    format & 3 == 2
}

/// Decodes `samples` samples of `format` from `bytes` (stereo interleaved
/// for PCM). ADPCM (mono only) is the GameCube/Wii DSP-ADPCM: 8-byte frames
/// of a header byte (coefficient pair in bits 4-6, scale exponent in bits
/// 0-3) and 14 4-bit samples, high nibble first; `history` is (y[n-1],
/// y[n-2]), updated.
fn decode(format: u16, bytes: &[u8], samples: usize, coefs: &[i16; 16], history: &mut (i16, i16)) -> Vec<[i16; 2]> {
    let mut out = Vec::with_capacity(samples);
    match (format >> 2) & 3 {
        0 => {
            let ch = if stereo(format) { 2 } else { 1 };
            for frame in bytes.chunks_exact(ch).take(samples) {
                let l = i16::from(frame[0] as i8) << 8;
                out.push([l, if ch == 2 { i16::from(frame[1] as i8) << 8 } else { l }]);
            }
        }
        1 => {
            let ch = if stereo(format) { 4 } else { 2 };
            for frame in bytes.chunks_exact(ch).take(samples) {
                let l = i16::from_le_bytes([frame[0], frame[1]]);
                out.push([l, if ch == 4 { i16::from_le_bytes([frame[2], frame[3]]) } else { l }]);
            }
        }
        _ => {
            'frames: for frame in bytes.as_chunks::<8>().0 {
                let header = frame[0];
                let pair = usize::from((header >> 4) & 7);
                let (c1, c2) = (i32::from(coefs[2 * pair]), i32::from(coefs[2 * pair + 1]));
                let scale = 1i32 << (header & 0xF);
                for k in 0..14 {
                    if out.len() == samples {
                        break 'frames;
                    }
                    let byte = frame[1 + k / 2];
                    let nibble = if k % 2 == 0 { byte >> 4 } else { byte & 0xF };
                    let nibble = i32::from(((nibble << 4) as i8) >> 4);
                    let predicted = (nibble * scale) << 11;
                    let y = ((predicted + 1024 + c1 * i32::from(history.0) + c2 * i32::from(history.1)) >> 11)
                        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                    *history = (y, history.0);
                    out.push([y, y]);
                }
            }
        }
    }
    out
}

/// Bytes holding `samples` samples of `format`.
fn byte_len(format: u16, samples: u32) -> u32 {
    let ch = if stereo(format) { 2 } else { 1 };
    match (format >> 2) & 3 {
        0 => samples * ch,
        1 => samples * 2 * ch,
        _ => samples.div_ceil(14) * 8,
    }
}

/// A source's filters (3dbrew "DSP Memory Region", source configuration):
/// a one-pole "simple" filter and a biquad, each enabled by a flag bit,
/// applied per output sample after interpolation, simple first (the order
/// follows Citra's HLE; 3dbrew does not give it). Coefficients are
/// fixed-point: simple s1.15 (b0, negated a1), biquad s2.14 (negated a2,
/// negated a1, b2, b1, b0).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Filters {
    simple: bool,
    biquad: bool,
    /// b0, -a1.
    simple_coefs: [f32; 2],
    /// -a2, -a1, b2, b1, b0.
    biquad_coefs: [f32; 5],
    /// Per channel: simple y[n-1]; biquad x[n-1], x[n-2], y[n-1], y[n-2].
    simple_state: [f32; 2],
    biquad_state: [[f32; 4]; 2],
}

impl Filters {
    fn apply(&mut self, sample: [f32; 2]) -> [f32; 2] {
        let mut out = sample;
        for (c, x) in out.iter_mut().enumerate() {
            if self.simple {
                let [b0, na1] = self.simple_coefs;
                let y = b0 * *x + na1 * self.simple_state[c];
                self.simple_state[c] = y;
                *x = y;
            }
            if self.biquad {
                let [na2, na1, b2, b1, b0] = self.biquad_coefs;
                let [x1, x2, y1, y2] = self.biquad_state[c];
                let y = b0 * *x + b1 * x1 + b2 * x2 + na1 * y1 + na2 * y2;
                self.biquad_state[c] = [*x, x1, y, y1];
                *x = y;
            }
        }
        out
    }
}

/// What a source needs to read and decode its buffers' samples.
#[derive(Clone, Copy)]
pub struct Samples<'a> {
    pub mem: &'a Memory,
    /// Where FCRAM's physical 0x20000000 is mapped (the linear heap).
    pub linear_base: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Source {
    enabled: bool,
    sync: u16,
    /// Queued buffers, played lowest ID first.
    queue: Vec<Buffer>,
    current: Option<Buffer>,
    /// Position in the current buffer, in samples.
    position: f64,
    current_id: u16,
    /// The ID of the buffer that last finished playing.
    finished_id: u16,
    /// A buffer after the first started since the last status.
    id_changed: bool,
    format: u16,
    coefs: [i16; 16],
    /// ADPCM history carried from one buffer to the next.
    history: (i16, i16),
    /// The current buffer, decoded (empty when its memory could not be
    /// read, or nothing is playing).
    decoded: Vec<[i16; 2]>,
    /// Gains: intermediate mix, then channel (front left, front right,
    /// back left, back right).
    gain: [[f32; 4]; 3],
    rate: f64,
    filters: Filters,
}

impl Source {
    fn enqueue(&mut self, b: Buffer) {
        let at = self.queue.partition_point(|q| q.id < b.id);
        self.queue.insert(at, b);
    }

    /// Takes the next buffer, if any; false when there is none.
    fn next(&mut self, samples: Option<Samples>) -> bool {
        if let Some(done) = self.current {
            self.finished_id = done.id;
        }
        if self.queue.is_empty() {
            self.current = None;
            return false;
        }
        let b = self.queue.remove(0);
        if self.current.is_some() {
            self.id_changed = true;
        }
        self.position = f64::from(b.start);
        self.current_id = b.id;
        self.current = Some(b);
        self.load(samples);
        true
    }

    /// Decodes the current buffer from memory.
    fn load(&mut self, samples: Option<Samples>) {
        self.decoded.clear();
        let (Some(b), Some(m)) = (self.current, samples) else { return };
        if let Some(h) = b.adpcm {
            self.history = h;
        }
        let va = match b.address {
            0x2000_0000..=0x27FF_FFFF => b.address - 0x2000_0000 + m.linear_base,
            _ => return,
        };
        let len = byte_len(self.format, b.samples);
        if let Ok(bytes) = m.mem.read_range(va, len) {
            let history = self.history;
            self.decoded = decode(self.format, &bytes, b.samples as usize, &self.coefs, &mut self.history);
            // USUM_DSP_TRACE=n: describe the first n buffers decoded
            static TRACED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let limit = std::env::var("USUM_DSP_TRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
            if TRACED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < limit {
                eprintln!(
                    "dsp buffer {} format {:#x} {} samples at {:#x} rate {:.3} gains {:?} filters {:?} history {history:?} coefs {:?} bytes {:02x?} decoded {:?}",
                    b.id,
                    self.format,
                    b.samples,
                    b.address,
                    self.rate,
                    self.gain,
                    (self.filters.simple, self.filters.simple_coefs, self.filters.biquad, self.filters.biquad_coefs),
                    self.coefs,
                    &bytes[..bytes.len().min(16)],
                    self.decoded.iter().take(12).map(|s| s[0]).collect::<Vec<_>>()
                );
            }
        }
    }

    /// Out of buffers: disabled, and the current buffer becomes ID 0.
    fn stop(&mut self) {
        if self.enabled {
            self.id_changed = true;
            self.current_id = 0;
        }
        self.enabled = false;
    }

    /// The sample at `position` of the current buffer, linearly
    /// interpolated, as -1..1.
    fn sample(&self) -> [f32; 2] {
        let i = self.position as usize;
        let Some(&a) = self.decoded.get(i) else { return [0.0; 2] };
        let b = self.decoded.get(i + 1).copied().unwrap_or(a);
        let t = (self.position - i as f64) as f32;
        [0, 1].map(|c| (f32::from(a[c]) * (1.0 - t) + f32::from(b[c]) * t) / 32768.0)
    }

    /// One frame: 160 output samples, each `rate` samples of the source
    /// on, mixed into `out` when given. A source whose buffers are all
    /// played disables itself (the end of a sound) and reports it as a
    /// buffer change to ID 0 (as Citra's HLE does; tested against the
    /// game, R-035).
    fn advance(&mut self, samples: Option<Samples>, mut out: Option<&mut [[f32; 2]; 160]>) {
        if !self.enabled {
            return;
        }
        if self.current.is_none() && !self.next(samples) {
            self.stop();
            return;
        }
        let gain = self.gain[0];
        let rate = if self.rate > 0.0 { self.rate } else { 1.0 };
        let mut k = 0;
        while k < 160 {
            let Some(b) = self.current else { break };
            if self.position >= f64::from(b.samples) {
                if b.looping && b.samples > 0 {
                    self.position -= f64::from(b.samples);
                    // an ADPCM loop starts again from the buffer's context
                    if b.adpcm.is_some() {
                        self.load(samples);
                    }
                    continue;
                }
                if !self.next(samples) {
                    self.stop();
                    return;
                }
                continue;
            }
            if let Some(out) = out.as_deref_mut() {
                let [l, r] = self.filters.apply(self.sample());
                out[k][0] += l * gain[0];
                out[k][1] += r * gain[1];
            }
            self.position += rate;
            k += 1;
        }
    }

    /// The 12-byte status: enabled, buffer ID changed, sync, position
    /// (halfwords high first), current buffer ID, and at +10 the ID of the
    /// buffer that last finished (3dbrew leaves +10 undescribed; Citra's
    /// HLE fills it so, a hypothesis tested against the game, R-035).
    fn status(&mut self) -> [u8; 12] {
        let mut s = [0u8; 12];
        s[0] = self.enabled as u8;
        s[1] = self.id_changed as u8;
        s[2..4].copy_from_slice(&self.sync.to_le_bytes());
        s[4..8].copy_from_slice(&to_dsp32(self.position as u32));
        s[8..10].copy_from_slice(&self.current_id.to_le_bytes());
        s[10..12].copy_from_slice(&self.finished_id.to_le_bytes());
        self.id_changed = false;
        s
    }
}

/// A u32 stored as two little-endian halfwords, high first.
pub fn from_dsp32(b: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes([b[0], b[1]])) << 16 | u32::from(u16::from_le_bytes([b[2], b[3]]))
}

pub fn to_dsp32(v: u32) -> [u8; 4] {
    let [a, b] = ((v >> 16) as u16).to_le_bytes();
    let [c, d] = (v as u16).to_le_bytes();
    [a, b, c, d]
}

/// A 16-bit stereo WAV file of `samples` at the DSP's output rate.
pub fn wav(samples: &[[i16; 2]]) -> Vec<u8> {
    let data = (samples.len() * 4) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // PCM
    out.extend(2u16.to_le_bytes()); // channels
    out.extend(OUTPUT_RATE.to_le_bytes());
    out.extend((OUTPUT_RATE * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for [l, r] in samples {
        out.extend(l.to_le_bytes());
        out.extend(r.to_le_bytes());
    }
    out
}

/// The index of the current region: the higher frame counter, wrapping.
pub fn current_region(counters: [u16; 2]) -> usize {
    usize::from((counters[1].wrapping_sub(counters[0]) as i16) > 0)
}

#[derive(Debug, Clone, Default)]
pub struct Dsp {
    sources: Vec<Source>,
    pub frames: u64,
    /// Mixed output, 32,728 stereo samples a second, for a consumer to
    /// drain (the window's audio, the probe's WAV); at most two seconds
    /// are kept.
    pub output: std::collections::VecDeque<[i16; 2]>,
    /// Live play: each frame's samples go straight to this queue as they
    /// are mixed (at most `sink_max` kept, oldest dropped), instead of
    /// `output`.
    pub sink: Option<(Queue, usize)>,
    /// Everything mixed, when recording (the probe's USUM_WAV).
    pub record: Option<Vec<[i16; 2]>>,
    /// Source changes, for the probe's report: (frame, source, playing,
    /// current buffer ID, its samples, the dirty flags), a change from the
    /// source's previous entry only, the latest 2000.
    pub events: std::collections::VecDeque<(u64, usize, bool, u16, u32, u32)>,
    last_event: Vec<(bool, u16, u32, u32)>,
}

fn structure(index: usize) -> u32 {
    dsp::STRUCTURE_SIZES[..index].iter().sum()
}

impl Dsp {
    /// One audio frame. Errors are guest memory faults in the regions.
    /// `linear_base` is where FCRAM's physical start is mapped.
    pub fn frame(&mut self, mem: &mut Memory, linear_base: u32) -> Result<(), String> {
        if self.sources.len() != SOURCES {
            self.sources = vec![Source::default(); SOURCES];
        }
        let read16 = |m: &Memory, a: u32| m.read_u16(a).map_err(|f| format!("DSP region: {f:?}"));
        let counters = [read16(mem, REGIONS[0])?, read16(mem, REGIONS[1])?];
        let read = REGIONS[current_region(counters)];
        let write = REGIONS[1 - current_region(counters)];
        let (configs, statuses, coefficients) = (structure(1), structure(2), structure(3));
        let mut mix = [[0f32; 2]; 160];
        for (i, source) in self.sources.iter_mut().enumerate() {
            let at = read + configs + CONFIG_SIZE * i as u32;
            let mut c = mem.read_range(at, CONFIG_SIZE).map_err(|f| format!("DSP source {i}: {f:?}"))?;
            let flags = u32::from_le_bytes(c[0..4].try_into().unwrap());
            let queue_dirty = u16::from_le_bytes([c[74], c[75]]);
            if flags & dirty::RESET != 0 {
                *source = Source::default();
            }
            if flags & dirty::ENABLE != 0 {
                source.enabled = c[160] != 0;
            }
            // Ultra Sun never sets the format bits (0, 1) but always sends
            // the format with the embedded buffer (R-039)
            if flags & (dirty::FORMAT | dirty::MONO_OR_STEREO | dirty::EMBEDDED_BUFFER) != 0 {
                source.format = u16::from_le_bytes([c[180], c[181]]);
            }
            if flags & dirty::ADPCM_COEFFICIENTS != 0 {
                let at = read + coefficients + 32 * i as u32;
                let raw = mem.read_range(at, 32).map_err(|f| format!("DSP coefficients {i}: {f:?}"))?;
                source.coefs = std::array::from_fn(|k| i16::from_le_bytes([raw[2 * k], raw[2 * k + 1]]));
            }
            if flags & dirty::RATE != 0 {
                let rate = f32::from_le_bytes(c[52..56].try_into().unwrap());
                source.rate = if rate.is_finite() && rate > 0.0 { f64::from(rate) } else { 1.0 };
            }
            // gains for mixes 0-2: dirty bits 25-27
            for m in 0..3 {
                if flags & (1 << (25 + m)) != 0 {
                    source.gain[m] = std::array::from_fn(|ch| {
                        let at = 4 + 16 * m + 4 * ch;
                        f32::from_le_bytes(c[at..at + 4].try_into().unwrap())
                    });
                }
            }
            if flags & dirty::FILTERS_ENABLED != 0 {
                let enabled = u16::from_le_bytes([c[58], c[59]]);
                source.filters.simple = enabled & 1 != 0;
                source.filters.biquad = enabled & 2 != 0;
            }
            let i16_at = |at: usize| f32::from(i16::from_le_bytes([c[at], c[at + 1]]));
            if flags & dirty::SIMPLE_FILTER != 0 {
                source.filters.simple_coefs = [i16_at(60) / 32768.0, i16_at(62) / 32768.0];
            }
            if flags & dirty::BIQUAD_FILTER != 0 {
                source.filters.biquad_coefs = std::array::from_fn(|k| i16_at(64 + 2 * k) / 16384.0);
            }
            if flags & dirty::SYNC != 0 {
                source.sync = u16::from_le_bytes([c[162], c[163]]);
            }
            if flags & dirty::EMBEDDED_BUFFER != 0 {
                let start = if flags & dirty::PLAY_POSITION != 0 { from_dsp32(&c[164..168]) } else { 0 };
                source.enqueue(Buffer {
                    id: u16::from_le_bytes([c[190], c[191]]),
                    samples: from_dsp32(&c[176..180]),
                    looping: c[188] & 2 != 0,
                    start,
                    address: from_dsp32(&c[172..176]),
                    adpcm: (c[188] & 1 != 0)
                        .then(|| (i16::from_le_bytes([c[184], c[185]]), i16::from_le_bytes([c[186], c[187]]))),
                });
            }
            if flags & dirty::BUFFER_QUEUE != 0 {
                for q in (0..4).filter(|q| queue_dirty & (1 << q) != 0) {
                    let b = &c[76 + 20 * q..96 + 20 * q];
                    source.enqueue(Buffer {
                        id: u16::from_le_bytes([b[16], b[17]]),
                        samples: from_dsp32(&b[4..8]),
                        looping: b[15] != 0,
                        start: 0,
                        address: from_dsp32(&b[0..4]),
                        adpcm: (b[14] != 0).then(|| (i16::from_le_bytes([b[10], b[11]]), i16::from_le_bytes([b[12], b[13]]))),
                    });
                }
            }
            source.advance(Some(Samples { mem, linear_base }), Some(&mut mix));
            let event =
                (source.enabled && source.current.is_some(), source.current_id, source.current.map_or(0, |b| b.samples), flags);
            if self.last_event.len() != SOURCES {
                self.last_event = vec![(false, 0, 0, 0); SOURCES];
            }
            if self.last_event[i] != event {
                self.last_event[i] = event;
                self.events.push_back((self.frames, i, event.0, event.1, event.2, event.3));
                if self.events.len() > 2000 {
                    self.events.pop_front();
                }
            }
            // the DSP clears what it has taken
            if flags != 0 || queue_dirty != 0 {
                c[0..4].fill(0);
                c[74..76].fill(0);
                mem.device_write(at, &c).map_err(|f| format!("DSP source {i}: {f:?}"))?;
            }
            mem.device_write(write + statuses + STATUS_SIZE * i as u32, &source.status())
                .map_err(|f| format!("DSP status {i}: {f:?}"))?;
        }
        let pcm = mix.map(|lr| lr.map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16));
        if let Some(record) = &mut self.record {
            record.extend_from_slice(&pcm);
        }
        match &self.sink {
            Some((queue, max)) => {
                if let Ok(mut q) = queue.lock() {
                    q.extend(pcm);
                    if q.len() > *max {
                        let excess = q.len() - max;
                        q.drain(..excess);
                    }
                }
            }
            None => self.output.extend(pcm),
        }
        let keep = 2 * OUTPUT_RATE as usize;
        if self.output.len() > keep {
            self.output.drain(..self.output.len() - keep);
        }
        self.frames += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsp32_is_high_halfword_first() {
        assert_eq!(from_dsp32(&[0x96, 0x23, 0xE0, 0xF1]), 0x2396_F1E0, "measured: an FCRAM address");
        assert_eq!(from_dsp32(&[0, 0, 0, 0x38]), 0x3800);
        assert_eq!(from_dsp32(&to_dsp32(0x1234_5678)), 0x1234_5678);
    }

    #[test]
    fn current_region_is_the_higher_counter_with_wrap() {
        assert_eq!(current_region([63414, 63413]), 0);
        assert_eq!(current_region([4, 5]), 1);
        assert_eq!(current_region([0, 0xFFFF]), 0, "0 follows 0xFFFF");
    }

    #[test]
    fn a_sound_plays_its_buffers_then_stops() {
        let b = |id, samples, looping| Buffer { id, samples, looping, start: 0, address: 0, adpcm: None };
        let mut s = Source { enabled: true, ..Source::default() };
        s.enqueue(b(2, 200, false));
        s.enqueue(b(1, 100, false));
        s.advance(None, None);
        let st = s.status();
        assert_eq!((st[0], st[1], u16::from_le_bytes([st[8], st[9]])), (1, 1, 2), "buffer 1 done, 2 playing");
        assert_eq!(from_dsp32(&st[4..8]), 60);
        s.advance(None, None);
        let st = s.status();
        assert_eq!((st[0], st[1], u16::from_le_bytes([st[8], st[9]])), (0, 1, 0), "all played: disabled, buffer 0");
        assert_eq!(u16::from_le_bytes([st[10], st[11]]), 2, "2 finished last");
    }

    #[test]
    fn adpcm_and_pcm_decode() {
        // one ADPCM frame: coefficient pair 0 = (2048, 0), so y = x * scale
        // + y[n-1]; scale 2^0; nibbles 1, -1, 7, 0...
        let mut coefs = [0i16; 16];
        coefs[0] = 2048;
        let frame = [0x00, 0x1F, 0x70, 0, 0, 0, 0, 0];
        let mut h = (0, 0);
        let out = decode(0b1000, &frame, 4, &coefs, &mut h);
        let left: Vec<i16> = out.iter().map(|s| s[0]).collect();
        assert_eq!(left, [1, 0, 7, 7], "y = nibble + y[n-1]");
        assert_eq!(h, (7, 7));
        // PCM16 stereo, PCM8 mono
        assert_eq!(decode(0b0110, &[1, 0, 2, 0], 1, &coefs, &mut h), [[1, 2]]);
        assert_eq!(decode(0b0001, &[0x80], 1, &coefs, &mut h), [[-32768, -32768]]);
        assert_eq!((byte_len(0b1001, 15), byte_len(0b0110, 3)), (16, 12));
    }

    #[test]
    fn a_source_mixes_by_its_gain() {
        let mut s = Source { enabled: true, rate: 1.0, ..Source::default() };
        s.gain[0] = [0.5, 0.25, 0.0, 0.0];
        s.enqueue(Buffer { id: 1, samples: 400, looping: false, start: 0, address: 0, adpcm: None });
        s.next(None);
        s.decoded = vec![[16384, 16384]; 400];
        let mut out = [[0f32; 2]; 160];
        s.advance(None, Some(&mut out));
        assert_eq!(out[10], [0.25, 0.125]);
        assert_eq!(s.position, 160.0);
    }

    #[test]
    fn filters_follow_their_coefficients() {
        let mut f = Filters { simple: true, simple_coefs: [0.5, 0.5], ..Filters::default() };
        // one-pole: y = 0.5 x + 0.5 y[n-1]
        assert_eq!(f.apply([1.0, 1.0]), [0.5, 0.5]);
        assert_eq!(f.apply([1.0, 1.0]), [0.75, 0.75]);
        // a biquad passing x straight through (b0 = 1)
        let mut f = Filters { biquad: true, biquad_coefs: [0.0, 0.0, 0.0, 0.0, 1.0], ..Filters::default() };
        assert_eq!(f.apply([0.25, -0.5]), [0.25, -0.5]);
        // and one delaying by a sample (b1 = 1)
        let mut f = Filters { biquad: true, biquad_coefs: [0.0, 0.0, 0.0, 1.0, 0.0], ..Filters::default() };
        assert_eq!(f.apply([0.25, 0.0]), [0.0, 0.0]);
        assert_eq!(f.apply([0.0, 0.0]), [0.25, 0.0]);
    }

    #[test]
    fn looping_buffers_never_end() {
        let mut s = Source { enabled: true, ..Source::default() };
        s.enqueue(Buffer { id: 1, samples: 100, looping: true, start: 0, address: 0, adpcm: None });
        for _ in 0..10 {
            s.advance(None, None);
        }
        assert!(s.enabled);
    }
}
