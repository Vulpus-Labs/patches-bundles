//! Configuration for the slot deck.

use crate::window_buffer::is_power_of_two;

/// Configuration for an `OverlapBuffer` / `ProcessorHandle` pair.
///
/// All three parameters must be powers of two and non-zero.
/// `window_size` must be >= `overlap_factor`.
pub struct SlotDeckConfig {
    /// Length of each analysis/synthesis window in samples.
    pub window_size: usize,
    /// Number of overlapping windows; `hop_size = window_size / overlap_factor`.
    pub overlap_factor: usize,
    /// Audio-clock samples the processor is allowed before a result is considered late.
    pub processing_budget: usize,
}

impl SlotDeckConfig {
    /// Construct and validate a `SlotDeckConfig`.
    ///
    /// Returns `Err` if any parameter is zero, not a power of two, or if
    /// `window_size < overlap_factor`.
    pub fn new(
        window_size: usize,
        overlap_factor: usize,
        processing_budget: usize,
    ) -> Result<Self, &'static str> {
        if !is_power_of_two(window_size) {
            return Err("window_size must be a non-zero power of two");
        }
        if !is_power_of_two(overlap_factor) {
            return Err("overlap_factor must be a non-zero power of two");
        }
        if !is_power_of_two(processing_budget) {
            return Err("processing_budget must be a non-zero power of two");
        }
        if window_size < overlap_factor {
            return Err("window_size must be >= overlap_factor");
        }
        Ok(Self { window_size, overlap_factor, processing_budget })
    }

    /// `window_size / overlap_factor`
    pub fn hop_size(&self) -> usize {
        self.window_size / self.overlap_factor
    }

    /// `window_size + processing_budget`
    pub fn total_latency(&self) -> usize {
        self.window_size + self.processing_budget
    }

    /// Recommended slot pool size per direction.
    ///
    /// `next_power_of_two(2 * overlap_factor + pipeline_slots)` where
    /// `pipeline_slots = max(1, processing_budget / hop_size)`.
    pub fn pool_size(&self) -> usize {
        let pipeline_slots = (self.processing_budget / self.hop_size()).max(1);
        let raw = 2 * self.overlap_factor + pipeline_slots;
        raw.next_power_of_two()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_power_of_two() {
        assert!(SlotDeckConfig::new(2048, 3, 128).is_err());
        assert!(SlotDeckConfig::new(2000, 4, 128).is_err());
        assert!(SlotDeckConfig::new(2048, 4, 100).is_err());
    }

    #[test]
    fn rejects_zero_values() {
        assert!(SlotDeckConfig::new(0, 4, 128).is_err());
        assert!(SlotDeckConfig::new(2048, 0, 128).is_err());
        assert!(SlotDeckConfig::new(2048, 4, 0).is_err());
    }

    #[test]
    fn rejects_window_smaller_than_overlap() {
        assert!(SlotDeckConfig::new(4, 8, 128).is_err());
    }

    #[test]
    fn derived_values() {
        let cfg = SlotDeckConfig::new(2048, 4, 128).expect("valid config");
        assert_eq!(cfg.hop_size(), 512);
        assert_eq!(cfg.total_latency(), 2176);
        assert_eq!(cfg.pool_size(), 16);
    }
}
