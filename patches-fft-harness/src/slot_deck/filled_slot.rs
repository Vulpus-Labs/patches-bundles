/// A completed window ready for transfer between threads.
///
/// The same buffer circulates through the system: filling (audio thread) →
/// processing (processing thread) → draining (audio thread) → filling again.
pub struct FilledSlot {
    /// Sample position of the first sample in `data`.
    pub start: u64,
    /// Owned buffer of `window_size` samples.
    pub data: Box<[f32]>,
}
