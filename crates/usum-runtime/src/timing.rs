//! Host presentation policy and measurements, independent of guest scheduling.
//!
//! A GSP framebuffer submission is an update, not proof of a unique rendered
//! image. A host present is a successful submission to the window system, not
//! proof of physical scanout. Neither counter advances the guest clock.

use std::{collections::VecDeque, fmt, num::NonZeroU32, str::FromStr, time::Duration};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationRate {
    /// Present when notified, without an additional host cap.
    #[default]
    Auto,
    Limited(NonZeroU32),
}

impl FromStr for PresentationRate {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        match value.parse::<u32>() {
            Ok(hz @ 1..=1000) => Ok(Self::Limited(NonZeroU32::new(hz).unwrap())),
            _ => Err("presentation rate must be auto or 1..1000 presents/second"),
        }
    }
}

impl fmt::Display for PresentationRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Limited(hz) => write!(f, "{hz}"),
        }
    }
}

impl PresentationRate {
    fn period(self) -> Duration {
        match self {
            Self::Auto => Duration::ZERO,
            Self::Limited(hz) => Duration::from_nanos(1_000_000_000u64.div_ceil(u64::from(hz.get()))),
        }
    }

    pub fn next_preset(self) -> Self {
        let next = match self {
            Self::Auto => 30,
            Self::Limited(hz) => match hz.get() {
                30 => 60,
                60 => 120,
                120 => 144,
                144 => 240,
                _ => return Self::Auto,
            },
        };
        Self::Limited(NonZeroU32::new(next).unwrap())
    }
}

/// Coalesces window wakeups without delaying the game thread or accumulating
/// catch-up presents. Times are relative to a frontend's monotonic start.
#[derive(Debug, Default)]
pub struct PresentationPacer {
    rate: PresentationRate,
    pending: bool,
    queued: bool,
    last_attempt: Option<Duration>,
}

impl PresentationPacer {
    pub fn new(rate: PresentationRate) -> Self {
        Self { rate, ..Self::default() }
    }

    pub fn rate(&self) -> PresentationRate {
        self.rate
    }

    pub fn set_rate(&mut self, rate: PresentationRate) {
        self.rate = rate;
        self.request();
    }

    pub fn request(&mut self) {
        self.pending = true;
    }

    pub fn deadline(&self) -> Option<Duration> {
        (self.pending && !self.queued)
            .then(|| self.last_attempt.map_or(Duration::ZERO, |last| last.saturating_add(self.rate.period())))
    }

    fn due(&self, now: Duration) -> bool {
        self.last_attempt.is_none_or(|last| now.saturating_sub(last) >= self.rate.period())
    }

    pub fn queue(&mut self, now: Duration) -> bool {
        if !self.pending || self.queued || !self.due(now) {
            return false;
        }
        self.queued = true;
        true
    }

    /// Also handles unsolicited expose/resize redraws. They must respect the
    /// same cap, rather than creating a second source of presents.
    pub fn begin(&mut self, now: Duration) -> bool {
        self.queued = false;
        self.pending = true;
        self.due(now)
    }

    /// Pass the attempt's start, so drawing time is part of the period rather
    /// than an additional delay. A slow renderer remains throughput-limited.
    pub fn finish(&mut self, attempt_started: Duration) {
        self.last_attempt = Some(attempt_started);
        self.pending = false;
        self.queued = false;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStamp {
    pub guest_ns: u64,
    pub vblanks: u64,
    /// GSP's new-framebuffer flags, counted separately for each screen.
    pub updates: [u64; 2],
}

/// Frames and their versions travel under the same frontend mutex. Overwriting
/// a mailbox does not stop guest rendering, and failed captures do not relabel
/// an older image as a newly displayed update.
pub struct FrameMailbox<T> {
    pub frames: [Option<T>; 2],
    pub stamp: FrameStamp,
    pub versions: [u64; 2],
}

impl<T> Default for FrameMailbox<T> {
    fn default() -> Self {
        Self { frames: [None, None], stamp: FrameStamp::default(), versions: [0; 2] }
    }
}

impl<T> FrameMailbox<T> {
    pub fn publish(&mut self, frames: [Option<T>; 2], updates: [bool; 2], guest_ns: u64) {
        self.stamp.vblanks += 1;
        self.publish_native(frames, updates, guest_ns);
    }

    /// Publish submitted native frames without inventing a hardware VBlank.
    pub fn publish_native(&mut self, frames: [Option<T>; 2], updates: [bool; 2], guest_ns: u64) {
        self.stamp.guest_ns = guest_ns;
        for (screen, frame) in frames.into_iter().enumerate() {
            self.stamp.updates[screen] += u64::from(updates[screen]);
            if frame.is_some() {
                self.frames[screen] = frame;
                self.versions[screen] = self.stamp.updates[screen];
            }
        }
    }

    pub fn take(&mut self) -> ([Option<T>; 2], [u64; 2]) {
        ([self.frames[0].take(), self.frames[1].take()], self.versions)
    }
}

/// All rates use measured wall time. Interval/draw statistics cover at most the
/// latest 240 successful presents, including long gaps, without reading pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameSample {
    pub wall: Duration,
    pub stamp: FrameStamp,
    pub presents: u64,
    pub shown_updates: u64,
    pub repeats: u64,
    pub skipped_updates: u64,
    pub updates_per_second: f64,
    pub presents_per_second: f64,
    pub shown_per_second: f64,
    pub game_speed: f64,
    pub interval_mean_ms: f64,
    pub interval_p95_ms: f64,
    pub draw_mean_ms: f64,
}

impl fmt::Display for FrameSample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "timing: wall_ns={} guest_ns={} vblanks={} top_updates={} bottom_updates={} presents={} shown_updates={} repeats={} skipped_updates={} updates_s={:.2} presents_s={:.2} shown_s={:.2} speed={:.4} interval_mean_ms={:.3} interval_p95_ms={:.3} draw_mean_ms={:.3}",
            self.wall.as_nanos(), self.stamp.guest_ns, self.stamp.vblanks, self.stamp.updates[0], self.stamp.updates[1],
            self.presents, self.shown_updates, self.repeats, self.skipped_updates, self.updates_per_second,
            self.presents_per_second, self.shown_per_second, self.game_speed, self.interval_mean_ms,
            self.interval_p95_ms, self.draw_mean_ms)
    }
}

#[derive(Default)]
pub struct FrameTelemetry {
    presents: u64,
    shown: u64,
    repeats: u64,
    skipped: u64,
    displayed_top: u64,
    last_present: Option<Duration>,
    intervals: VecDeque<Duration>,
    draw_times: VecDeque<Duration>,
    previous: FrameSample,
}

impl FrameTelemetry {
    pub fn presented(&mut self, now: Duration, draw_time: Duration, top_version: u64) {
        if let Some(last) = self.last_present {
            Self::push(&mut self.intervals, now.saturating_sub(last));
        }
        self.last_present = Some(now);
        Self::push(&mut self.draw_times, draw_time);
        self.presents += 1;
        if top_version > self.displayed_top {
            self.shown += 1;
            self.skipped += top_version - self.displayed_top - 1;
            self.displayed_top = top_version;
        } else {
            self.repeats += 1;
        }
    }

    fn push(samples: &mut VecDeque<Duration>, value: Duration) {
        if samples.len() == 240 {
            samples.pop_front();
        }
        samples.push_back(value);
    }

    pub fn report(&mut self, now: Duration, stamp: FrameStamp) -> Option<FrameSample> {
        if now.saturating_sub(self.previous.wall) < Duration::from_secs(1) {
            return None;
        }
        let sample = self.sample(now, stamp, self.previous);
        self.previous = sample;
        Some(sample)
    }

    pub fn summary(&self, now: Duration, stamp: FrameStamp) -> FrameSample {
        self.sample(now, stamp, FrameSample::default())
    }

    fn sample(&self, now: Duration, stamp: FrameStamp, previous: FrameSample) -> FrameSample {
        let seconds = now.saturating_sub(previous.wall).as_secs_f64().max(1e-9);
        let mean_ms = |values: &VecDeque<Duration>| {
            values.iter().map(Duration::as_secs_f64).sum::<f64>() * 1000.0 / values.len().max(1) as f64
        };
        let mut intervals: Vec<_> = self.intervals.iter().copied().collect();
        intervals.sort_unstable();
        let p95 = intervals.get((intervals.len() * 95).div_ceil(100).saturating_sub(1)).copied().unwrap_or_default();
        FrameSample {
            wall: now,
            stamp,
            presents: self.presents,
            shown_updates: self.shown,
            repeats: self.repeats,
            skipped_updates: self.skipped,
            updates_per_second: stamp.updates[0].saturating_sub(previous.stamp.updates[0]) as f64 / seconds,
            presents_per_second: (self.presents - previous.presents) as f64 / seconds,
            shown_per_second: (self.shown - previous.shown_updates) as f64 / seconds,
            game_speed: stamp.guest_ns.saturating_sub(previous.stamp.guest_ns) as f64 / 1e9 / seconds,
            interval_mean_ms: mean_ms(&self.intervals),
            interval_p95_ms: p95.as_secs_f64() * 1000.0,
            draw_mean_ms: mean_ms(&self.draw_times),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_coalesce_without_catch_up_and_switch_without_a_guest_clock() {
        let mut pacer = PresentationPacer::new("30".parse().unwrap());
        pacer.request();
        assert!(pacer.queue(Duration::ZERO));
        assert!(!pacer.queue(Duration::ZERO));
        assert!(pacer.begin(Duration::ZERO));
        pacer.finish(Duration::ZERO);
        for _ in 0..100 {
            pacer.request();
        }
        assert!(!pacer.queue(Duration::from_millis(33)));
        assert!(pacer.queue(Duration::from_millis(34)));
        assert!(pacer.begin(Duration::from_secs(4)));
        pacer.finish(Duration::from_secs(4));
        pacer.request();
        assert!(!pacer.begin(Duration::from_secs(4))); // unsolicited redraw cannot evade the cap
        assert_eq!(pacer.deadline(), Some(Duration::from_secs(4) + PresentationRate::from_str("30").unwrap().period()));
        pacer.set_rate(PresentationRate::Auto);
        assert!(pacer.queue(Duration::from_secs(4)));
        assert!(pacer.begin(Duration::from_secs(4)));
        pacer.finish(Duration::from_secs(4));
        assert_eq!(pacer.deadline(), None); // no duplicate redraw loop
        for bad in ["0", "1001", "-1", "nan", "unlimited"] {
            assert!(bad.parse::<PresentationRate>().is_err());
        }
        assert_eq!(" AUTO ".parse::<PresentationRate>().unwrap(), PresentationRate::Auto);
        let mut rate = PresentationRate::Auto;
        for expected in ["30", "60", "120", "144", "240", "auto"] {
            rate = rate.next_preset();
            assert_eq!(rate.to_string(), expected);
        }
    }

    #[test]
    fn presentation_cadence_is_independent_of_frontend_epoch_and_includes_draw_time() {
        for origin in [Duration::ZERO, Duration::from_secs(7)] {
            let mut pacer = PresentationPacer::new("30".parse().unwrap());
            for offset in [0, 34, 68, 9000] {
                let start = origin + Duration::from_millis(offset);
                pacer.request();
                assert!(pacer.queue(start));
                assert!(pacer.begin(start));
                pacer.finish(start);
                pacer.request();
                assert!(!pacer.queue(start + Duration::from_millis(8)));
                assert_eq!(pacer.deadline(), Some(start + PresentationRate::from_str("30").unwrap().period()));
            }
            // No extra cap delay when a redraw itself exceeds the period.
            let start = origin + Duration::from_secs(10);
            assert!(pacer.queue(start));
            assert!(pacer.begin(start));
            pacer.finish(start);
            pacer.request();
            assert!(pacer.queue(start + Duration::from_millis(40)));
            assert!(!pacer.queue(start + Duration::from_millis(40)));
        }
        let timing = FrameTelemetry::default();
        let sample = timing.summary(Duration::from_secs(6), FrameStamp { guest_ns: 1_000_000_000, ..FrameStamp::default() });
        assert_eq!(sample.game_speed, 1.0 / 6.0, "session averages include startup before the guest clock runs");
    }

    #[test]
    fn mailbox_keeps_versions_with_frames_across_overwrites_and_failed_captures() {
        let mut mailbox = FrameMailbox::default();
        mailbox.publish([Some(11), Some(21)], [true, true], 16);
        mailbox.publish([None, Some(22)], [true, false], 32); // top capture failed
        assert_eq!(mailbox.take(), ([Some(11), Some(22)], [1, 1]));
        assert_eq!(mailbox.stamp.updates, [2, 1]);
        mailbox.publish([Some(13), None], [true, false], 48);
        mailbox.publish([Some(14), None], [true, false], 64);
        assert_eq!(mailbox.take(), ([Some(14), None], [4, 1]));
        assert_eq!(mailbox.stamp, FrameStamp { guest_ns: 64, vblanks: 4, updates: [4, 1] });
    }

    #[test]
    fn statistics_separate_game_time_updates_repeats_and_skipped_presentations() {
        let mut timing = FrameTelemetry::default();
        timing.presented(Duration::from_millis(100), Duration::from_millis(2), 1);
        timing.presented(Duration::from_millis(116), Duration::from_millis(4), 1);
        timing.presented(Duration::from_millis(216), Duration::from_millis(6), 4);
        let stamp = FrameStamp { guest_ns: 500_000_000, vblanks: 30, updates: [4, 2] };
        assert!(timing.report(Duration::from_millis(999), stamp).is_none());
        let sample = timing.report(Duration::from_secs(1), stamp).unwrap();
        assert_eq!((sample.presents, sample.shown_updates, sample.repeats, sample.skipped_updates), (3, 2, 1, 2));
        assert_eq!((sample.updates_per_second, sample.presents_per_second, sample.shown_per_second), (4.0, 3.0, 2.0));
        assert_eq!(sample.game_speed, 0.5);
        assert_eq!((sample.interval_mean_ms, sample.interval_p95_ms, sample.draw_mean_ms), (58.0, 100.0, 4.0));
        let stalled = timing.report(Duration::from_secs(3), stamp).unwrap();
        assert_eq!((stalled.updates_per_second, stalled.presents_per_second, stalled.game_speed), (0.0, 0.0, 0.0));
        assert_eq!(timing.summary(Duration::from_secs(3), stamp).presents, 3);
        for n in 0..300 {
            timing.presented(Duration::from_secs(4) + Duration::from_millis(n * 10), Duration::from_millis(1), 5 + n);
        }
        assert_eq!(timing.intervals.len(), 240);
        assert_eq!(timing.draw_times.len(), 240);
    }
}
