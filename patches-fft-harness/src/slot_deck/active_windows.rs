//! Fixed-capacity array of active windowed buffers with position-aware operations.
//!
//! Both the filling side (writing samples into windows) and the draining side
//! (overlap-add reading) share the same underlying data structure: a fixed-size
//! array of `Option<(start_position, buffer)>` entries, with operations that
//! address buffers by their absolute sample position.

use super::filled_slot::FilledSlot;

/// A fixed-capacity set of active windowed buffers.
///
/// Each entry is a `(start, buffer)` pair where `start` is the absolute sample
/// position of the first sample in the buffer. The buffer has `window_size`
/// samples.
///
/// At most one window can complete or expire per sample tick, because each
/// active window has a unique `start` (spaced by `hop_size`).
pub(crate) struct ActiveWindows {
    slots: Vec<Option<(u64, Box<[f32]>)>>,
    window_size: usize,
}

impl ActiveWindows {
    /// Create an `ActiveWindows` with `capacity` slots, all initially empty.
    pub fn new(capacity: usize, window_size: usize) -> Self {
        Self {
            slots: vec![None; capacity],
            window_size,
        }
    }

    /// The window size for buffers in this set.
    pub fn window_size(&self) -> usize {
        self.window_size
    }

    /// Insert a buffer at the given start position.
    ///
    /// Returns the buffer back if no free slot is available.
    pub fn insert(&mut self, start: u64, buf: Box<[f32]>) -> Option<Box<[f32]>> {
        if let Some(entry) = self.slots.iter_mut().find(|e| e.is_none()) {
            *entry = Some((start, buf));
            None
        } else {
            Some(buf)
        }
    }

    /// Write `sample` into every active window at `position`.
    ///
    /// Returns the one completed buffer (as a `FilledSlot`) if any window's
    /// last sample was just written. At most one window can complete per
    /// sample, since each has a unique start.
    pub fn write_sample(&mut self, position: u64, sample: f32) -> Option<FilledSlot> {
        let window_size = self.window_size;
        let mut completed = None;

        for entry in self.slots.iter_mut() {
            let (start, data) = match entry {
                Some(inner) => inner,
                None => continue,
            };
            let start_val = *start;
            let offset = position.wrapping_sub(start_val);
            if offset < window_size as u64 {
                data[offset as usize] = sample;
                // Last sample in the window — extract as completed.
                if offset == window_size as u64 - 1 {
                    let (s, buf) = entry.take().unwrap();
                    completed = Some(FilledSlot { start: s, data: buf });
                }
            }
        }

        completed
    }

    /// Sum contributions from all active windows at `position`.
    ///
    /// Returns the overlap-add sum and the one expired buffer (if any window
    /// has reached or passed its last sample). At most one window can expire
    /// per sample, since each has a unique start.
    pub fn read_sum(&mut self, position: u64) -> (f32, Option<Box<[f32]>>) {
        let window_size = self.window_size;
        let mut sum = 0.0f32;
        let mut expired = None;

        for entry in self.slots.iter_mut() {
            let start = match entry {
                Some((s, _)) => *s,
                None => continue,
            };
            // wrapping_sub gives a huge value when position < start, which is
            // >= window_size and therefore contributes nothing — no branch needed.
            let offset = position.wrapping_sub(start);
            if offset < window_size as u64 {
                sum += entry.as_ref().unwrap().1[offset as usize];
            }
            // Expire when this is the last (or a past) sample of the window.
            if position + 1 >= start + window_size as u64 {
                let (_, buf) = entry.take().unwrap();
                expired = Some(buf);
            }
        }

        (sum, expired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buf(window_size: usize, fill: f32) -> Box<[f32]> {
        vec![fill; window_size].into_boxed_slice()
    }

    #[test]
    fn insert_into_empty() {
        let mut aw = ActiveWindows::new(2, 4);
        let rejected = aw.insert(0, make_buf(4, 0.0));
        assert!(rejected.is_none(), "should accept into empty slot");
    }

    #[test]
    fn insert_returns_buffer_when_full() {
        let mut aw = ActiveWindows::new(1, 4);
        assert!(aw.insert(0, make_buf(4, 0.0)).is_none());
        let rejected = aw.insert(4, make_buf(4, 1.0));
        assert!(rejected.is_some(), "should reject when full");
    }

    #[test]
    fn write_sample_fills_and_completes() {
        let mut aw = ActiveWindows::new(2, 4);
        aw.insert(0, make_buf(4, 0.0));

        // Write samples 0..3 — no completion yet.
        for pos in 0..3 {
            let completed = aw.write_sample(pos, (pos + 1) as f32);
            assert!(completed.is_none());
        }

        // Sample 3 is the last — should complete.
        let completed = aw.write_sample(3, 4.0);
        assert!(completed.is_some());
        let slot = completed.unwrap();
        assert_eq!(slot.start, 0);
        assert_eq!(&*slot.data, &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn write_sample_with_overlapping_windows() {
        let mut aw = ActiveWindows::new(2, 4);
        // Window A starts at 0, window B starts at 2 (hop=2).
        aw.insert(0, make_buf(4, 0.0));
        aw.insert(2, make_buf(4, 0.0));

        // Position 2 writes into both windows.
        let completed = aw.write_sample(2, 10.0);
        assert!(completed.is_none());

        // Position 3 completes window A (last sample at offset 3).
        let completed = aw.write_sample(3, 20.0);
        assert!(completed.is_some());
        let slot = completed.unwrap();
        assert_eq!(slot.start, 0);
        assert_eq!(slot.data[2], 10.0);
        assert_eq!(slot.data[3], 20.0);
    }

    #[test]
    fn read_sum_overlap_add() {
        let mut aw = ActiveWindows::new(2, 4);

        // Two overlapping windows with known data.
        let mut buf_a = make_buf(4, 0.0);
        buf_a[2] = 3.0;
        buf_a[3] = 5.0;
        aw.insert(0, buf_a);

        let mut buf_b = make_buf(4, 0.0);
        buf_b[0] = 7.0;
        buf_b[1] = 11.0;
        aw.insert(2, buf_b);

        // Position 2: offset 2 in A (3.0) + offset 0 in B (7.0) = 10.0
        let (sum, expired) = aw.read_sum(2);
        assert_eq!(sum, 10.0);
        assert!(expired.is_none());

        // Position 3: offset 3 in A (5.0) + offset 1 in B (11.0) = 16.0
        // Window A expires (position 3 is its last sample).
        let (sum, expired) = aw.read_sum(3);
        assert_eq!(sum, 16.0);
        assert!(expired.is_some());
    }

    #[test]
    fn read_sum_no_contribution_before_start() {
        let mut aw = ActiveWindows::new(1, 4);
        let mut buf = make_buf(4, 0.0);
        buf[0] = 99.0;
        aw.insert(10, buf);

        // Position 8 is before the window start — wrapping_sub gives huge offset.
        let (sum, expired) = aw.read_sum(8);
        assert_eq!(sum, 0.0);
        assert!(expired.is_none());
    }

    #[test]
    fn expiry_frees_slot_for_reuse() {
        let mut aw = ActiveWindows::new(1, 2);
        aw.insert(0, make_buf(2, 1.0));

        // Expire at position 1 (last sample of a 2-sample window starting at 0).
        let (_, expired) = aw.read_sum(1);
        assert!(expired.is_some());

        // Slot should now be free for a new insert.
        let rejected = aw.insert(2, make_buf(2, 2.0));
        assert!(rejected.is_none());
    }
}
