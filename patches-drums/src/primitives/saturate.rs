/// Soft-clipping saturation function.
///
/// `drive` in [0, 1] maps from clean (pass-through) to hard clip via tanh-like
/// curve. At drive = 0, output equals input (assuming input is in [-1, 1]).
/// At drive = 1, aggressive clipping.
#[inline]
pub fn saturate(sample: f32, drive: f32) -> f32 {
    if drive <= 0.0 {
        return sample;
    }
    // Scale input by 1 + drive * 4 to push into saturation region
    let gain = 1.0 + drive * 4.0;
    let x = sample * gain;
    // Fast tanh approximation
    patches_dsp::fast_tanh(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_within;

    #[test]
    fn saturate_unity_at_zero_drive() {
        // At zero drive, output should equal input
        for &x in &[-1.0, -0.5, 0.0, 0.5, 1.0] {
            let y = saturate(x, 0.0);
            assert_within!(x, y, 1e-6, "saturate({x}, 0) should be {x}, got {y}");
        }
    }

    #[test]
    fn saturate_symmetry() {
        for &drive in &[0.0, 0.3, 0.5, 0.7, 1.0] {
            for &x in &[0.1, 0.3, 0.5, 0.8, 1.0] {
                let pos = saturate(x, drive);
                let neg = saturate(-x, drive);
                assert_within!(
                    pos, -neg, 1e-6,
                    "saturate should be odd: f({x})={pos}, f(-{x})={neg}"
                );
            }
        }
    }

    #[test]
    fn saturate_bounded_output() {
        // With non-zero drive, output is bounded to [-1, 1] even for large inputs
        for &drive in &[0.3, 0.5, 1.0] {
            for &x in &[-2.0, -1.0, 0.0, 1.0, 2.0] {
                let y = saturate(x, drive);
                assert!(
                    (-1.01..=1.01).contains(&y),
                    "saturate({x}, {drive}) = {y} is out of [-1, 1]"
                );
            }
        }
        // At zero drive with input in [-1, 1], output is in [-1, 1]
        for &x in &[-1.0, -0.5, 0.0, 0.5, 1.0] {
            let y = saturate(x, 0.0);
            assert!(
                (-1.0..=1.0).contains(&y),
                "saturate({x}, 0) = {y} is out of [-1, 1]"
            );
        }
    }
}
