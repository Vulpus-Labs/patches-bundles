//! Precomputed windowing function buffer.

/// A precomputed window of fixed length, built from a windowing function.
///
/// The windowing function `f` maps a normalised position `n ∈ [0, 1)` to a
/// weight `w ∈ [0, 1]`. The buffer stores `w[i] = f(i / window_size)` for
/// `i = 0..window_size`.
///
/// For WOLA pipelines, use [`normalised_wola`](WindowBuffer::normalised_wola)
/// to produce a pre-normalised synthesis window from the analysis window.
pub struct WindowBuffer {
    coefficients: Box<[f32]>,
}

impl WindowBuffer {
    /// Create a window buffer of `window_size` samples from the given function.
    ///
    /// `f(n)` is called with `n = i as f32 / window_size as f32` for each
    /// sample index `i`.
    pub fn new(window_size: usize, f: impl Fn(f32) -> f32) -> Self {
        let coefficients = (0..window_size)
            .map(|i| f(i as f32 / window_size as f32))
            .collect::<Vec<f32>>()
            .into_boxed_slice();
        Self { coefficients }
    }

    /// Apply this window to `input`, writing `input[n] * w[n]` into `output[n]`.
    ///
    /// Both slices must have length `window_size`.
    pub fn apply_into(&self, input: &[f32], output: &mut [f32]) {
        assert_eq!(input.len(), self.coefficients.len(), "input length must match window size");
        assert_eq!(output.len(), self.coefficients.len(), "output length must match window size");
        for ((s, &w), o) in input.iter().zip(self.coefficients.iter()).zip(output.iter_mut()) {
            *o = s * w;
        }
    }

    /// Apply this window in-place: `data[n] *= w[n]`.
    ///
    /// Slice must have length `window_size`.
    pub fn apply(&self, data: &mut [f32]) {
        assert_eq!(data.len(), self.coefficients.len(), "data length must match window size");
        for (s, &w) in data.iter_mut().zip(self.coefficients.iter()) {
            *s *= w;
        }
    }

    /// Number of samples in the window.
    pub fn window_size(&self) -> usize {
        self.coefficients.len()
    }

    /// Create a pre-normalised WOLA synthesis window.
    ///
    /// Computes the reciprocal of the sum of squared window values at each
    /// hop phase, then multiplies each coefficient by the corresponding factor:
    ///
    /// ```text
    /// norm[p] = 1 / Σ_{k: p + k·hop < window_size}  w[p + k·hop]²
    /// synth[n] = w[n] * norm[n % hop_size]
    /// ```
    ///
    /// Since every window starts at a hop boundary, `(r - start) % hop` is
    /// the same for all contributing windows at position `r`, so the factor
    /// tiles uniformly. Applying this window during synthesis makes the
    /// overlap-add sum come out correct without any per-sample normalisation
    /// on the audio thread.
    ///
    /// Panics if `hop_size` is zero or not a power of two.
    pub fn normalised_wola(&self, hop_size: usize) -> WindowBuffer {
        assert!(hop_size > 0, "hop_size must be non-zero");
        assert!(is_power_of_two(hop_size), "hop_size must be a power of two");

        let window_size = self.coefficients.len();
        let hop_mask = hop_size - 1;

        // Compute reciprocal of the sum-of-squares at each hop phase.
        let reciprocals: Vec<f32> = (0..hop_size)
            .map(|phase| {
                let mut sum = 0.0f32;
                let mut i = phase;
                while i < window_size {
                    let w = self.coefficients[i];
                    sum += w * w;
                    i += hop_size;
                }
                if sum > 0.0 { 1.0 / sum } else { 0.0 }
            })
            .collect();

        // Multiply each window coefficient by the matching phase factor.
        let coefficients = self.coefficients
            .iter()
            .enumerate()
            .map(|(i, &w)| w * reciprocals[i & hop_mask])
            .collect::<Vec<f32>>()
            .into_boxed_slice();

        WindowBuffer { coefficients }
    }
}

pub(crate) fn is_power_of_two(n: usize) -> bool {
    n != 0 && n & (n - 1) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hann(n: f32) -> f32 {
        (std::f32::consts::PI * n).sin().powi(2)
    }

    #[test]
    fn window_size_matches_requested() {
        let w = WindowBuffer::new(64, |_| 1.0);
        assert_eq!(w.window_size(), 64);
    }

    #[test]
    fn coefficients_match_function() {
        let w = WindowBuffer::new(4, |n| n * 2.0);
        let mut out = [0.0f32; 4];
        let ones = [1.0f32; 4];
        w.apply_into(&ones, &mut out);
        // f(0/4)=0, f(1/4)=0.5, f(2/4)=1.0, f(3/4)=1.5
        assert_eq!(out, [0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn apply_into_multiplies() {
        let w = WindowBuffer::new(3, |_| 0.5);
        let input = [2.0, 4.0, 6.0];
        let mut output = [0.0f32; 3];
        w.apply_into(&input, &mut output);
        assert_eq!(output, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn apply_in_place() {
        let w = WindowBuffer::new(3, |_| 0.5);
        let mut data = [2.0, 4.0, 6.0];
        w.apply(&mut data);
        assert_eq!(data, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn hann_overlap_2_is_cola() {
        // Hann window with 50% overlap: sin²(x) + cos²(x) = 1 at every sample.
        let window_size = 32;
        let hop_size = 16;
        let w = WindowBuffer::new(window_size, hann);

        // Sum overlapping windows at each hop phase.
        for phase in 0..hop_size {
            let mut sum = 0.0f32;
            let mut i = phase;
            while i < window_size {
                let c = hann(i as f32 / window_size as f32);
                sum += c;
                i += hop_size;
            }
            assert!(
                (sum - 1.0).abs() < 1e-6,
                "COLA sum at phase {phase}: {sum}"
            );
        }

        // Verify normalised_wola doesn't panic.
        let _ = w.normalised_wola(hop_size);
    }

    #[test]
    fn normalised_wola_compensates_double_windowing() {
        // With overlap factor 4, Hann applied twice (analysis + synthesis) gives
        // w² = sin⁴, which is not COLA. normalised_wola should compensate so
        // that the overlap-add of w * synth sums to 1.
        let window_size = 32;
        let hop_size = 8; // overlap factor 4
        let w = WindowBuffer::new(window_size, hann);
        let synth = w.normalised_wola(hop_size);

        // Simulate WOLA: for each hop phase, sum analysis * synthesis across
        // all overlapping windows at that phase.
        let ones = vec![1.0f32; window_size];
        let mut analysis_out = vec![0.0f32; window_size];
        let mut synth_out = vec![0.0f32; window_size];
        w.apply_into(&ones, &mut analysis_out);
        synth.apply_into(&analysis_out, &mut synth_out);

        for phase in 0..hop_size {
            let mut sum = 0.0f32;
            let mut i = phase;
            while i < window_size {
                sum += synth_out[i];
                i += hop_size;
            }
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "WOLA sum at phase {phase}: {sum}"
            );
        }
    }
}
