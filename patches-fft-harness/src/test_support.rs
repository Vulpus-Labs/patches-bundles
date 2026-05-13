/// Assert that `actual` is within an absolute `delta` of `expected`.
///
/// Mirror of the `assert_within!` macro in `patches-dsp` /
/// `patches-core`, kept local so the harness keeps a narrow dep
/// surface.
macro_rules! assert_within {
    ($expected:expr, $actual:expr, $delta:expr) => {{
        let expected: f32 = $expected;
        let actual: f32 = $actual;
        let delta: f32 = $delta;
        assert!(
            (expected - actual).abs() < delta,
            "assert_within failed: expected {}, actual {}, delta {}",
            expected,
            actual,
            delta
        );
    }};
    ($expected:expr, $actual:expr, $delta:expr, $($arg:tt)+) => {{
        let expected: f32 = $expected;
        let actual: f32 = $actual;
        let delta: f32 = $delta;
        assert!(
            (expected - actual).abs() < delta,
            $($arg)+
        );
    }};
}

pub(crate) use assert_within;

fn bin_magnitude(packed: &[f32], bin: usize) -> f32 {
    if bin == 0 {
        packed[0].abs()
    } else if bin == packed.len() / 2 {
        packed[1].abs()
    } else {
        packed[2 * bin].hypot(packed[2 * bin + 1])
    }
}

/// Bin index (excluding DC and Nyquist) with the greatest magnitude in
/// a packed FFT buffer. `n` is the FFT size.
pub(crate) fn dominant_bin(packed: &[f32], n: usize) -> usize {
    let mut best_k = 1;
    let mut best_mag = 0.0f32;
    for k in 1..(n / 2) {
        let mag = bin_magnitude(packed, k);
        if mag > best_mag {
            best_mag = mag;
            best_k = k;
        }
    }
    best_k
}
