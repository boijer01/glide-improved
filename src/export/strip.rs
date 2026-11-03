//! The voice strip: a low cut, a three-band EQ, a noise gate, a compressor
//! and a limiter for the microphone. It runs on the whole microphone track
//! before the track is cut to the edit, so the preview and the export sound
//! the same, and an edit never changes how the voice is processed.

use super::audio::Stereo;
use crate::capture::mixer::RATE;
use crate::project::model::VoiceStrip;

/// Where the EQ's bass and air shelves turn.
const BASS_HZ: f64 = 120.0;
const AIR_HZ: f64 = 8000.0;
/// How far the gate turns a closed signal down.
const GATE_RANGE_DB: f64 = -40.0;
/// Compressor knee width.
const KNEE_DB: f64 = 6.0;

fn db_to_gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
fn coeff(ms: f64) -> f64 {
    (-1.0 / (RATE * ms / 1000.0)).exp()
}

/// A second-order filter (the RBJ audio EQ cookbook), coefficients divided
/// by a0.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Biquad { b0: b[0] / a[0], b1: b[1] / a[0], b2: b[2] / a[0], a1: a[1] / a[0], a2: a[2] / a[0] }
    }

    fn highpass(hz: f64) -> Self {
        let w = std::f64::consts::TAU * hz / RATE;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2));
        Biquad::new([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + alpha, -2.0 * c, 1.0 - alpha])
    }

    fn peak(hz: f64, q: f64, db: f64) -> Self {
        let a = 10f64.powf(db / 40.0);
        let w = std::f64::consts::TAU * hz / RATE;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Biquad::new([1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a], [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a])
    }

    /// A shelf below `hz` (`low`) or above it, with a slope of 1.
    fn shelf(hz: f64, db: f64, low: bool) -> Self {
        let a = 10f64.powf(db / 40.0);
        let w = std::f64::consts::TAU * hz / RATE;
        let (c, alpha) = (w.cos(), w.sin() / 2.0 * std::f64::consts::SQRT_2);
        let s = 2.0 * a.sqrt() * alpha;
        if low {
            Biquad::new(
                [a * ((a + 1.0) - (a - 1.0) * c + s), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - s)],
                [(a + 1.0) + (a - 1.0) * c + s, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - s],
            )
        } else {
            Biquad::new(
                [a * ((a + 1.0) + (a - 1.0) * c + s), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - s)],
                [(a + 1.0) - (a - 1.0) * c + s, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - s],
            )
        }
    }

    /// Filter both channels in place, each with its own state.
    fn run(&self, buf: &mut [Stereo]) {
        let mut z = [[0.0f64; 2]; 2];
        for s in buf.iter_mut() {
            for (ch, state) in z.iter_mut().enumerate() {
                let x = s[ch] as f64;
                let y = self.b0 * x + state[0];
                state[0] = self.b1 * x - self.a1 * y + state[1];
                state[1] = self.b2 * x - self.a2 * y;
                s[ch] = y as f32;
            }
        }
    }
}

/// Turn the signal down by `GATE_RANGE_DB` while it stays under `threshold_db`
/// (quick to open, slower to close, so words aren't cut off).
fn gate(buf: &mut [Stereo], threshold_db: f64) {
    let (env_release, open, close) = (coeff(50.0), coeff(2.0), coeff(120.0));
    let (limit, closed) = (db_to_gain(threshold_db), db_to_gain(GATE_RANGE_DB));
    let (mut env, mut g) = (0.0f64, 1.0f64);
    for s in buf.iter_mut() {
        let x = (s[0].abs().max(s[1].abs())) as f64;
        env = x.max(env * env_release);
        let target = if env >= limit { 1.0 } else { closed };
        let k = if target > g { open } else { close };
        g = target + (g - target) * k;
        s[0] = (s[0] as f64 * g) as f32;
        s[1] = (s[1] as f64 * g) as f32;
    }
}

/// Level after a compressor with a soft knee, for an input level in dB.
fn compressed(x: f64, threshold: f64, ratio: f64) -> f64 {
    let over = x - threshold;
    if 2.0 * over < -KNEE_DB {
        x
    } else if 2.0 * over <= KNEE_DB {
        x + (1.0 / ratio - 1.0) * (over + KNEE_DB / 2.0).powi(2) / (2.0 * KNEE_DB)
    } else {
        threshold + over / ratio
    }
}

/// Even out the level: anything over `threshold_db` is reduced by `ratio`,
/// then everything is raised by `makeup_db`. Both channels get the same
/// gain, so the voice doesn't move left or right.
fn compress(buf: &mut [Stereo], threshold_db: f64, ratio: f64, makeup_db: f64) {
    let (attack, release) = (coeff(10.0), coeff(150.0));
    let ratio = ratio.max(1.0);
    let mut env = 0.0f64;
    for s in buf.iter_mut() {
        let x = (s[0] as f64).powi(2).max((s[1] as f64).powi(2));
        let k = if x > env { attack } else { release };
        env = x + (env - x) * k;
        let level = 10.0 * env.max(1e-20).log10();
        let g = db_to_gain(compressed(level, threshold_db, ratio) - level + makeup_db);
        s[0] = (s[0] as f64 * g) as f32;
        s[1] = (s[1] as f64 * g) as f32;
    }
}

/// Keep every sample under `ceiling_db`. It works on the whole track at
/// once, so it turns down just before a peak and lets go after it, with no
/// delay added to the voice.
fn limit(buf: &mut [Stereo], ceiling_db: f64) {
    let ceiling = db_to_gain(ceiling_db);
    let (attack, release) = (coeff(1.5), coeff(80.0));
    // The most each sample may be played at.
    let mut g: Vec<f64> = buf.iter().map(|s| (ceiling / (s[0].abs().max(s[1].abs()) as f64).max(1e-12)).min(1.0)).collect();
    // Backwards: start turning down a little before each peak.
    for i in (0..g.len().saturating_sub(1)).rev() {
        let next = g[i + 1];
        g[i] = g[i].min(next + (1.0 - next) * (1.0 - attack));
    }
    // Forwards: let go slowly after it.
    for i in 1..g.len() {
        let prev = g[i - 1];
        g[i] = g[i].min(prev + (1.0 - prev) * (1.0 - release));
    }
    for (s, g) in buf.iter_mut().zip(g) {
        s[0] = (s[0] as f64 * g) as f32;
        s[1] = (s[1] as f64 * g) as f32;
    }
}

/// Run the strip over a microphone track: low cut, gate, EQ, compressor,
/// limiter, each only when it is switched on. Nothing changes while the strip
/// is off.
pub fn process(buf: &mut [Stereo], v: &VoiceStrip) {
    if !v.on {
        return;
    }
    if v.low_cut {
        Biquad::highpass(v.low_cut_hz.clamp(20.0, 400.0)).run(buf);
    }
    if v.gate {
        gate(buf, v.gate_db);
    }
    if v.eq {
        for (db, f) in [
            (v.bass_db, Biquad::shelf(BASS_HZ, v.bass_db, true)),
            (v.presence_db, Biquad::peak(v.presence_hz.clamp(500.0, 8000.0), 1.0, v.presence_db)),
            (v.air_db, Biquad::shelf(AIR_HZ, v.air_db, false)),
        ] {
            if db.abs() > 0.01 {
                f.run(buf);
            }
        }
    }
    if v.compressor {
        compress(buf, v.threshold_db, v.ratio, v.makeup_db);
    }
    if v.limiter {
        limit(buf, v.ceiling_db);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, amp: f64, secs: f64) -> Vec<Stereo> {
        (0..(secs * RATE) as usize)
            .map(|i| {
                let v = (amp * (std::f64::consts::TAU * hz * i as f64 / RATE).sin()) as f32;
                [v, v]
            })
            .collect()
    }

    /// RMS level (dB) of the second half, after filters and envelopes settle.
    fn level(buf: &[Stereo]) -> f64 {
        let half = &buf[buf.len() / 2..];
        let ms = half.iter().map(|s| (s[0] as f64).powi(2)).sum::<f64>() / half.len() as f64;
        10.0 * ms.max(1e-20).log10()
    }

    /// How much `f` changes the level of a sine at `hz` (dB).
    fn change(hz: f64, f: impl Fn(&mut [Stereo])) -> f64 {
        let mut b = sine(hz, 0.1, 1.0);
        let before = level(&b);
        f(&mut b);
        level(&b) - before
    }

    fn only(set: impl Fn(&mut VoiceStrip)) -> VoiceStrip {
        let mut v = VoiceStrip { on: true, low_cut: false, eq: false, gate: false, compressor: false, limiter: false, ..Default::default() };
        set(&mut v);
        v
    }

    #[test]
    fn low_cut_removes_rumble_and_keeps_the_voice() {
        let v = only(|v| {
            v.low_cut = true;
            v.low_cut_hz = 80.0;
        });
        assert!(change(25.0, |b| process(b, &v)) < -15.0);
        assert!(change(1000.0, |b| process(b, &v)).abs() < 0.2);
    }

    #[test]
    fn eq_bands_raise_and_lower_their_own_range() {
        let presence = only(|v| {
            v.eq = true;
            v.presence_db = 6.0;
            v.presence_hz = 3000.0;
        });
        assert!((change(3000.0, |b| process(b, &presence)) - 6.0).abs() < 0.3);
        assert!(change(200.0, |b| process(b, &presence)).abs() < 0.5);
        let bass = only(|v| {
            v.eq = true;
            v.bass_db = -6.0;
        });
        assert!((change(40.0, |b| process(b, &bass)) + 6.0).abs() < 0.6);
        assert!(change(3000.0, |b| process(b, &bass)).abs() < 0.3);
        let air = only(|v| {
            v.eq = true;
            v.air_db = 4.0;
        });
        assert!((change(16_000.0, |b| process(b, &air)) - 4.0).abs() < 0.6);
        assert!(change(500.0, |b| process(b, &air)).abs() < 0.3);
    }

    #[test]
    fn the_gate_silences_hiss_and_lets_the_voice_through() {
        let v = only(|v| {
            v.gate = true;
            v.gate_db = -40.0;
        });
        let mut hiss = sine(5000.0, 0.002, 1.0); // about -57 dB
        let before = level(&hiss);
        process(&mut hiss, &v);
        assert!(level(&hiss) - before < -35.0);
        let mut voice = sine(300.0, 0.2, 1.0);
        let before = level(&voice);
        process(&mut voice, &v);
        assert!((level(&voice) - before).abs() < 0.1);
    }

    #[test]
    fn the_compressor_tames_loud_parts_and_leaves_quiet_ones() {
        let v = only(|v| {
            v.compressor = true;
            v.threshold_db = -24.0;
            v.ratio = 3.0;
            v.makeup_db = 0.0;
        });
        // A sine at amplitude 0.5 peaks at -6 dB: 18 dB over, so it comes out 12 dB lower.
        assert!((change(300.0, |b| {
            for s in b.iter_mut() {
                s[0] *= 5.0;
                s[1] *= 5.0;
            }
            process(b, &v)
        }) - (20.0 * 5f64.log10() - 12.0))
            .abs()
            < 0.8);
        // Well under the threshold nothing happens, and makeup raises everything.
        let mut quiet = sine(300.0, 0.005, 1.0);
        let before = level(&quiet);
        process(&mut quiet, &v);
        assert!((level(&quiet) - before).abs() < 0.05);
        let lifted = only(|v| {
            v.compressor = true;
            v.threshold_db = -24.0;
            v.ratio = 3.0;
            v.makeup_db = 6.0;
        });
        assert!((change(300.0, |b| {
            for s in b.iter_mut() {
                s[0] *= 0.05;
                s[1] *= 0.05;
            }
            process(b, &lifted)
        }) - (20.0 * 0.05f64.log10() + 6.0))
            .abs()
            < 0.1);
    }

    #[test]
    fn the_limiter_never_lets_a_sample_over_the_ceiling() {
        let v = only(|v| {
            v.limiter = true;
            v.ceiling_db = -3.0;
        });
        let mut b = sine(200.0, 0.3, 1.0);
        // A burst at full scale in the middle.
        for s in &mut b[20_000..21_000] {
            s[0] *= 3.3;
            s[1] *= 3.3;
        }
        process(&mut b, &v);
        let ceiling = db_to_gain(-3.0) as f32 + 1e-6;
        assert!(b.iter().all(|s| s[0].abs() <= ceiling && s[1].abs() <= ceiling));
        // Far from the burst the quiet sine is untouched.
        let tail = &b[40_000..];
        let peak = tail.iter().fold(0.0f32, |m, s| m.max(s[0].abs()));
        assert!((peak - 0.3).abs() < 0.001, "{peak}");
    }

    #[test]
    fn an_off_strip_changes_nothing() {
        let original = sine(440.0, 0.3, 0.2);
        let mut b = original.clone();
        process(&mut b, &VoiceStrip { on: false, ..Default::default() });
        assert_eq!(b, original);
        // On, but every part off: nothing either.
        let mut c = original.clone();
        process(&mut c, &only(|_| {}));
        assert_eq!(c, original);
    }

    #[test]
    fn the_default_strip_is_a_sensible_voice_chain() {
        let v = VoiceStrip { on: true, ..Default::default() };
        assert!(v.low_cut && v.eq && v.compressor && v.limiter && !v.gate);
        // A very loud voice stays under the ceiling.
        let mut loud = sine(300.0, 0.95, 1.0);
        process(&mut loud, &v);
        let ceiling = db_to_gain(v.ceiling_db) as f32 + 1e-6;
        assert!(loud.iter().all(|s| s[0].abs() <= ceiling));
        // A voice at a usual level (about -23 dB) comes out a little louder,
        // from the makeup gain, and loud and quiet voices end up closer.
        let quiet_change = change(300.0, |b| process(b, &v));
        assert!(quiet_change > 0.0 && quiet_change < 5.0, "{quiet_change}");
        let mut a = sine(300.0, 0.4, 1.0);
        let mut b = sine(300.0, 0.04, 1.0);
        let gap_before = level(&a) - level(&b);
        process(&mut a, &v);
        process(&mut b, &v);
        assert!(level(&a) - level(&b) < gap_before - 6.0, "{} -> {}", gap_before, level(&a) - level(&b));
    }
}
