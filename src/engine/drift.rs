//! Drift: a slow push-in that keeps the picture from standing still. Each
//! calm stretch (a drift effect's range with the zooms taken out) starts a
//! new push-in, so it moves in the gaps between the zooms and gives way to
//! them. Pure functions of time, so the preview and the export agree.

/// A push-in starts this long after a zoom has ended, when the zoom's own
/// movement has settled.
const SETTLE_MS: f64 = 300.0;
/// A push-in eases back over this long before a zoom or the end of the
/// effect, so the picture doesn't jump to its resting size.
const EASE_OUT_MS: f64 = 500.0;
/// Stretches shorter than this stay still; a push-in over so little time
/// would be a twitch.
const MIN_STRETCH_MS: f64 = 1500.0;
/// A stretch this long gets the full amount; shorter ones get less, so the
/// speed never gets much higher.
const FULL_AT_MS: f64 = 6000.0;

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The calm stretches of `start..end` (ms): what is left when the `zooms`
/// are taken out, each push-in starting `SETTLE_MS` after a zoom ends.
/// Stretches that would be too short are left out.
pub fn calm_stretches(start: f64, end: f64, zooms: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut zooms: Vec<(f64, f64)> = zooms.iter().copied().filter(|&(a, b)| b > a && b > start && a < end).collect();
    zooms.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = vec![];
    let mut from = start;
    for (a, b) in zooms {
        if a > from {
            out.push((from, a));
        }
        from = from.max(b + SETTLE_MS);
    }
    if end > from {
        out.push((from, end));
    }
    out.retain(|&(a, b)| b - a >= MIN_STRETCH_MS);
    out
}

/// How much bigger than at rest the picture is at time `t` (ms): 1 outside
/// the effect and in zooms, up to `1 + amount` at the end of a calm stretch.
pub fn scale(t: f64, start: f64, end: f64, zooms: &[(f64, f64)], amount: f64) -> f64 {
    if t < start || t > end {
        return 1.0;
    }
    let Some((a, b)) = calm_stretches(start, end, zooms).into_iter().find(|&(a, b)| t >= a && t <= b) else {
        return 1.0;
    };
    let grow = ((t - a) / (b - a - EASE_OUT_MS)).clamp(0.0, 1.0);
    let back = (t - (b - EASE_OUT_MS)) / EASE_OUT_MS;
    let reach = amount * ((b - a) / FULL_AT_MS).min(1.0);
    1.0 + reach * smoothstep(grow) * (1.0 - smoothstep(back))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMOUNT: f64 = 0.12;

    #[test]
    fn a_single_stretch_pushes_in_and_eases_back_at_its_end() {
        let at = |t| scale(t, 0.0, 10_000.0, &[], AMOUNT);
        assert_eq!(at(0.0), 1.0);
        let (early, mid, late) = (at(1500.0), at(4500.0), at(8000.0));
        assert!(1.0 < early && early < mid && mid < late, "{early} {mid} {late}");
        // It reaches the full amount just before the ease back, and rests again at the end.
        assert!((at(9500.0) - (1.0 + AMOUNT)).abs() < 1e-9);
        assert!(at(9800.0) < at(9500.0));
        assert!((at(10_000.0) - 1.0).abs() < 1e-9);
        // Outside the effect nothing happens.
        assert_eq!(scale(-1.0, 0.0, 10_000.0, &[], AMOUNT), 1.0);
        assert_eq!(scale(10_001.0, 0.0, 10_000.0, &[], AMOUNT), 1.0);
    }

    #[test]
    fn every_calm_stretch_starts_over_and_zooms_are_left_alone() {
        let zooms = [(8_000.0, 10_000.0)];
        let at = |t| scale(t, 0.0, 20_000.0, &zooms, AMOUNT);
        // Rested at the zoom's start and all through it.
        assert!((at(8_000.0) - 1.0).abs() < 1e-9);
        assert_eq!(at(9_000.0), 1.0);
        assert_eq!(at(10_100.0), 1.0, "still settling");
        // A new push-in after it, not the old one picking up where it was.
        assert!(at(12_000.0) > 1.0);
        assert!(at(12_000.0) < at(17_000.0));
        assert!(at(5_000.0) > 1.0, "and before it");
    }

    #[test]
    fn short_stretches_stay_still_and_medium_ones_move_less() {
        let zooms = [(1_000.0, 2_000.0), (3_000.0, 9_000.0)];
        // 2.3..3 s is too short; 9.3..10 s too.
        assert_eq!(calm_stretches(0.0, 10_000.0, &zooms), vec![]);
        // A 3 s stretch reaches half the amount.
        let peak = scale(2_500.0, 0.0, 3_000.0, &[], AMOUNT);
        assert!((peak - (1.0 + AMOUNT * 0.5)).abs() < 1e-9, "{peak}");
    }

    #[test]
    fn zooms_may_overlap_come_unsorted_or_stick_out_of_the_effect() {
        let zooms = [(9_000.0, 12_000.0), (4_000.0, 6_000.0), (5_000.0, 7_000.0)];
        let found = calm_stretches(2_000.0, 11_000.0, &zooms);
        assert_eq!(found, vec![(2_000.0, 4_000.0), (7_300.0, 9_000.0)]);
        // Entirely outside the effect: ignored.
        assert_eq!(calm_stretches(0.0, 5_000.0, &[(8_000.0, 9_000.0)]), vec![(0.0, 5_000.0)]);
    }

    #[test]
    fn it_never_makes_the_picture_smaller() {
        for i in 0..400 {
            let t = i as f64 * 50.0;
            assert!(scale(t, 500.0, 19_000.0, &[(6_000.0, 8_000.0)], 0.2) >= 1.0);
        }
    }
}
