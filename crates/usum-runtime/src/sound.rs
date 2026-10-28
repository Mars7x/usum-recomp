//! Sound for the play window: the DSP's mixed output played through the
//! PulseAudio native protocol (`pulseaudio`, pure Rust), on the session's
//! socket. PipeWire serves the same protocol; in a Flatpak the socket is
//! granted with `--socket=pulseaudio`.
//!
//! The server clocks the stream: it asks for bytes, and gets whatever the
//! game has mixed since, padded with silence when the game is behind (it
//! is, while the renderer runs slower than the console).

use std::collections::VecDeque;
use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::net::UnixStream;

use pulseaudio::protocol;

/// Samples waiting to be played; the game pushes, the server pulls.
pub use crate::audio::Queue;

/// At most this many samples wait (120 ms), so the sound never lags far
/// behind the picture.
pub const MAX_QUEUED: usize = crate::audio::OUTPUT_RATE as usize * 120 / 1000;

/// The server-side buffer asked for, sink latency included: 50 ms of
/// 16-bit stereo. Left to itself the server picks about 2 s, heard as the
/// sound lagging the picture. 60 ms crackled while the game delivered
/// sound in bursts; since it streams each frame as mixed, with idle time
/// paced to the wall clock, 50 ms holds (R-039).
const TARGET_BYTES: u32 = crate::audio::OUTPUT_RATE * 4 * 50 / 1000;

/// After running dry, playback resumes only once this much is queued
/// (30 ms): one short gap instead of a click at every burst.
const RESUME: usize = crate::audio::OUTPUT_RATE as usize * 30 / 1000;

/// Samples over which a gap fades the last sample out (about 2 ms).
const FADE: usize = 64;

/// The server's side of the queue: what was last played, and whether it
/// is waiting for the queue to refill.
#[derive(Default)]
struct Player {
    last: [i16; 2],
    fade: usize,
    starved: bool,
    /// Position between the queue's first two samples.
    frac: f64,
    /// The queue's fill, smoothed (samples).
    fill: f64,
}

/// The fill the playback rate steers towards (30 ms).
const SETPOINT: f64 = crate::audio::OUTPUT_RATE as f64 * 0.030;

/// Playback rate for a smoothed fill: 1 at the setpoint, at most 0.5%
/// slower as the queue empties or faster as it fills (dynamic rate
/// control). That absorbs small drifts between the game and the sound
/// device without an audible change of pitch. A wider range (down to
/// 0.85) covered more of the game's slow scenes but was heard as music
/// out of tune (R-048); now a scene slower than that has short gaps.
fn rate(fill: f64) -> f64 {
    (1.0 + 0.005 * (fill - SETPOINT) / SETPOINT).clamp(0.995, 1.005)
}

impl Player {
    /// The next `frames` samples: from the queue while it lasts; a short
    /// fade to silence when it runs dry, then silence until `RESUME`
    /// samples are queued again. Returns the bytes and how many samples
    /// were filled in.
    fn take(&mut self, q: &mut VecDeque<[i16; 2]>, frames: usize) -> (Vec<u8>, usize) {
        let mut data = Vec::with_capacity(frames * 4);
        let mut filled = 0;
        if self.starved && q.len() >= RESUME {
            self.starved = false;
        }
        self.fill += (q.len() as f64 - self.fill) * 0.05;
        let step = rate(self.fill);
        for _ in 0..frames {
            let sample = match (self.starved, q.len() >= 2) {
                (false, true) => {
                    self.fade = FADE;
                    let (a, b) = (q[0], q[1]);
                    let t = self.frac as f32;
                    let s = [0, 1].map(|c| (f32::from(a[c]) + (f32::from(b[c]) - f32::from(a[c])) * t) as i16);
                    self.frac += step;
                    while self.frac >= 1.0 && !q.is_empty() {
                        q.pop_front();
                        self.frac -= 1.0;
                    }
                    s
                }
                _ => {
                    self.starved = true;
                    filled += 1;
                    self.fade = self.fade.saturating_sub(1);
                    let scale = self.fade as f32 / FADE as f32;
                    self.last.map(|v| (f32::from(v) * scale) as i16)
                }
            };
            if !self.starved {
                self.last = sample;
            }
            data.extend(sample[0].to_le_bytes());
            data.extend(sample[1].to_le_bytes());
        }
        (data, filled)
    }
}

/// Adds the game's new samples, dropping the oldest beyond `MAX_QUEUED`.
pub fn push(queue: &Queue, samples: impl IntoIterator<Item = [i16; 2]>) {
    if let Ok(mut q) = queue.lock() {
        q.extend(samples);
        if q.len() > MAX_QUEUED {
            let excess = q.len() - MAX_QUEUED;
            q.drain(..excess);
        }
    }
}

fn connect() -> Result<(BufReader<UnixStream>, u16), String> {
    let path = pulseaudio::socket_path_from_env().ok_or("no PulseAudio socket")?;
    let mut sock = BufReader::new(UnixStream::connect(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
    let e = |e: protocol::ProtocolError| e.to_string();
    protocol::write_command_message(sock.get_mut(), 0, &protocol::Command::Auth(auth), protocol::MAX_VERSION).map_err(e)?;
    let (_, reply) = protocol::read_reply_message::<protocol::AuthReply>(&mut sock, protocol::MAX_VERSION).map_err(e)?;
    let version = protocol::MAX_VERSION.min(reply.version);
    let mut props = protocol::Props::new();
    props.set(protocol::Prop::ApplicationName, CString::new("usum-recomp").expect("no NUL"));
    protocol::write_command_message(sock.get_mut(), 1, &protocol::Command::SetClientName(props), version).map_err(e)?;
    protocol::read_reply_message::<protocol::SetClientNameReply>(&mut sock, version).map_err(e)?;
    Ok((sock, version))
}

/// Plays `queue` until the connection fails or `quit` is set; returns why
/// it stopped.
pub fn play(queue: &Queue, quit: &std::sync::atomic::AtomicBool) -> String {
    let mut stats = (0usize, 0usize);
    let rate = f64::from(crate::audio::OUTPUT_RATE);
    let outcome = stream(queue, quit, &mut stats);
    let summary =
        format!("{:.1} s played, {:.1} s of it silence filling in for the game", stats.0 as f64 / rate, stats.1 as f64 / rate);
    match outcome {
        Ok(()) => format!("sound stopped: {summary}"),
        Err(e) => format!("no sound: {e} ({summary})"),
    }
}

/// `stats`: samples sent, and how many of them were silence because the
/// game had not mixed them yet.
fn stream(queue: &Queue, quit: &std::sync::atomic::AtomicBool, stats: &mut (usize, usize)) -> Result<(), String> {
    let e = |e: protocol::ProtocolError| e.to_string();
    let (mut sock, version) = connect()?;
    let params = protocol::PlaybackStreamParams {
        sample_spec: protocol::SampleSpec {
            format: protocol::SampleFormat::S16Le,
            channels: 2,
            sample_rate: crate::audio::OUTPUT_RATE,
        },
        channel_map: protocol::ChannelMap::stereo(),
        cvolume: Some(protocol::ChannelVolume::norm(2)),
        sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
        buffer_attr: protocol::stream::BufferAttr {
            max_length: u32::MAX,
            target_length: TARGET_BYTES,
            pre_buffering: u32::MAX,
            minimum_request_length: u32::MAX,
            fragment_size: u32::MAX,
        },
        flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
        ..Default::default()
    };
    protocol::write_command_message(sock.get_mut(), 2, &protocol::Command::CreatePlaybackStream(params), version).map_err(e)?;
    let (_, info) = protocol::read_reply_message::<protocol::CreatePlaybackStreamReply>(&mut sock, version).map_err(e)?;
    let mut player = Player::default();
    let mut send = |sock: &mut BufReader<UnixStream>, bytes: usize| -> Result<(), String> {
        let frames = bytes / 4;
        let (data, filled) = match queue.lock() {
            Ok(mut q) => player.take(&mut q, frames),
            Err(_) => (vec![0; frames * 4], frames),
        };
        stats.0 += frames;
        stats.1 += filled;
        protocol::write_memblock(sock.get_mut(), info.channel, &data, 0).map_err(e)
    };
    send(&mut sock, info.requested_bytes as usize)?;
    while !quit.load(std::sync::atomic::Ordering::Relaxed) {
        let (_, message) = protocol::read_command_message(&mut sock, version).map_err(e)?;
        match message {
            protocol::Command::Request(protocol::Request { channel, length }) if channel == info.channel => {
                send(&mut sock, length as usize)?;
            }
            protocol::Command::Error(err) => return Err(format!("server error {err:?}")),
            // started, underflow (the game behind) and the like
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_dry_queue_fades_then_waits_to_refill() {
        let mut p = Player::default();
        let mut q: VecDeque<[i16; 2]> = vec![[1000, 1000]; 10].into();
        let (data, filled) = p.take(&mut q, 20);
        // the queued samples play (stretched a little: the queue is low),
        // then the rest is filled in
        let played = 20 - filled;
        assert!((9..20).contains(&played), "{played} played");
        let sample = |i: usize| i16::from_le_bytes([data[4 * i], data[4 * i + 1]]);
        assert_eq!(sample(played - 1), 1000);
        assert!(sample(played) < 1000 && sample(played) > 0, "fading, not cut");
        // less than RESUME queued: still waiting
        q.extend([[5, 5]; 10]);
        let before = q.len();
        assert_eq!(p.take(&mut q, 4).1, 4);
        assert_eq!(q.len(), before, "nothing taken while refilling");
        q.extend(vec![[5, 5]; RESUME]);
        assert_eq!(p.take(&mut q, 4).1, 0, "resumed");
    }

    #[test]
    fn the_rate_follows_the_fill() {
        assert_eq!(rate(SETPOINT), 1.0);
        assert_eq!(rate(0.0), 0.995);
        assert_eq!(rate(10.0 * SETPOINT), 1.005);
        // at rate 1 the samples pass through unchanged
        let mut p = Player { fill: SETPOINT, ..Player::default() };
        let mut q: VecDeque<[i16; 2]> = (0..2000).map(|i| [i as i16, -(i as i16)]).collect();
        let (data, filled) = p.take(&mut q, 10);
        assert_eq!(filled, 0);
        let sample = |i: usize| i16::from_le_bytes([data[4 * i], data[4 * i + 1]]);
        assert_eq!((0..10).map(sample).collect::<Vec<_>>(), (0..10).collect::<Vec<i16>>());
    }

    #[test]
    fn the_queue_keeps_only_the_newest_samples() {
        let q: Queue = Arc::default();
        push(&q, (0..MAX_QUEUED as i16 + 10).map(|i| [i, i]));
        let q = q.lock().unwrap();
        assert_eq!(q.len(), MAX_QUEUED);
        assert_eq!(q.front(), Some(&[10, 10]));
    }
}
