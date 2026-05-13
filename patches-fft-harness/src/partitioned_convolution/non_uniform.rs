use super::convolver::PartitionedConvolver;
use super::ir_partitions::IrPartitions;

/// A single tier in a [`NonUniformConvolver`].
///
/// Each tier handles a contiguous segment of the IR at a specific block size.
/// Tier 0 uses the base block size; subsequent tiers double. The tier
/// accumulates base-block-sized input chunks in `input_ring` until a full
/// tier block is ready, then runs its inner `PartitionedConvolver`.
pub(super) struct ConvolutionTier {
    convolver: PartitionedConvolver,
    /// This tier's block size (power of two, ≥ base block size).
    pub(super) tier_block_size: usize,
    /// Ratio of this tier's block size to the base block size.
    ratio: usize,
    /// Ring buffer accumulating input samples (length = tier_block_size).
    input_ring: Box<[f32]>,
    /// Current write position in `input_ring` (0..tier_block_size).
    input_pos: usize,
    /// Ring buffer holding this tier's output contribution (length = tier_block_size).
    output_ring: Box<[f32]>,
    /// Current read position in `output_ring` (advances by base_block_size per call).
    output_read_pos: usize,
}

impl ConvolutionTier {
    fn new(ir_segment: &[f32], tier_block_size: usize) -> Self {
        let parts = IrPartitions::from_ir(ir_segment, tier_block_size);
        let convolver = PartitionedConvolver::new(parts);
        let ratio = 1; // set by caller
        Self {
            convolver,
            tier_block_size,
            ratio,
            input_ring: vec![0.0f32; tier_block_size].into_boxed_slice(),
            input_pos: 0,
            output_ring: vec![0.0f32; tier_block_size].into_boxed_slice(),
            output_read_pos: 0,
        }
    }

    fn reset(&mut self) {
        self.convolver.reset();
        self.input_ring.fill(0.0);
        self.input_pos = 0;
        self.output_ring.fill(0.0);
        self.output_read_pos = 0;
    }
}

/// Non-uniform partitioned convolver.
///
/// Splits the impulse response into geometrically-growing tiers. Tier 0 uses
/// `base_block_size`; each subsequent tier doubles, up to `max_tier_block_size`.
/// Larger tiers process less frequently, reducing total work from O(P × N) to
/// approximately O(N × log P).
///
/// All buffers are pre-allocated at construction. `process_block` performs zero
/// heap allocations.
///
/// # Usage
///
/// Call [`process_block`](Self::process_block) once per `base_block_size` input
/// samples, exactly as with [`PartitionedConvolver`].
pub struct NonUniformConvolver {
    pub(super) tiers: Vec<ConvolutionTier>,
    base_block_size: usize,
    /// Scratch buffer for tier convolver output (length = max tier block size).
    tier_output_scratch: Box<[f32]>,
}

impl NonUniformConvolver {
    /// Create a non-uniform convolver for the given IR.
    ///
    /// - `base_block_size`: input block size (power of 2, ≥ 2). This is the
    ///   granularity at which audio arrives.
    /// - `max_tier_block_size`: largest tier block size (power of 2, ≥
    ///   `base_block_size`). Tiers double from `base_block_size` up to this cap.
    ///
    /// # Panics
    ///
    /// Panics if `base_block_size` or `max_tier_block_size` is not a power of 2,
    /// if `max_tier_block_size < base_block_size`, or if `ir` is empty.
    pub fn new(ir: &[f32], base_block_size: usize, max_tier_block_size: usize) -> Self {
        assert!(base_block_size >= 2 && base_block_size.is_power_of_two());
        assert!(max_tier_block_size >= base_block_size && max_tier_block_size.is_power_of_two());
        assert!(!ir.is_empty());

        let mut tiers = Vec::new();
        let mut ir_offset = 0;
        let mut tier_block = base_block_size;

        while ir_offset < ir.len() {
            if tier_block < max_tier_block_size {
                // This tier covers one block's worth of IR at the current size.
                let end = (ir_offset + tier_block).min(ir.len());
                let segment = &ir[ir_offset..end];
                let mut tier = ConvolutionTier::new(segment, tier_block);
                tier.ratio = tier_block / base_block_size;
                tiers.push(tier);
                ir_offset = end;
                tier_block *= 2;
            } else {
                // Final tier: all remaining IR at max_tier_block_size.
                let segment = &ir[ir_offset..];
                let mut tier = ConvolutionTier::new(segment, max_tier_block_size);
                tier.ratio = max_tier_block_size / base_block_size;
                tiers.push(tier);
                break;
            }
        }

        let max_tier = tiers.iter().map(|t| t.tier_block_size).max().unwrap_or(base_block_size);
        Self {
            tiers,
            base_block_size,
            tier_output_scratch: vec![0.0f32; max_tier].into_boxed_slice(),
        }
    }

    /// Process one block of `base_block_size` input samples.
    ///
    /// # Panics
    ///
    /// Panics if `input.len()` or `output.len()` does not equal `base_block_size`.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32]) {
        let n = self.base_block_size;
        assert_eq!(input.len(), n);
        assert_eq!(output.len(), n);

        output.fill(0.0);

        for tier in &mut self.tiers {
            // Accumulate input into this tier's ring.
            tier.input_ring[tier.input_pos..tier.input_pos + n].copy_from_slice(input);
            tier.input_pos += n;

            // If the input ring is full, process a tier block.
            if tier.input_pos >= tier.tier_block_size {
                tier.input_pos = 0;
                let scratch = &mut self.tier_output_scratch[..tier.tier_block_size];
                tier.convolver.process_block(&tier.input_ring, scratch);
                // Copy result into the tier's output ring for reading.
                tier.output_ring.copy_from_slice(scratch);
                tier.output_read_pos = 0;
            }

            // Add this tier's contribution to output.
            let read = tier.output_read_pos;
            for (out, &tier_val) in output.iter_mut().zip(&tier.output_ring[read..read + n]) {
                *out += tier_val;
            }
            tier.output_read_pos += n;
        }
    }

    /// Serialize the pre-computed frequency-domain data into a flat `Vec<f32>`.
    ///
    /// The format is a private contract between `process_file` and
    /// `from_pre_fft`. Layout:
    ///
    /// ```text
    /// [tier_count, base_block_size]
    /// Per tier: [tier_block_size, partition_count, ratio, <partition_data...>]
    /// ```
    ///
    /// All header values are stored as `f32` (they are small integers).
    pub fn serialize_pre_fft(ir: &[f32], base_block_size: usize, max_tier_block_size: usize) -> Vec<f32> {
        // Build the convolver to partition the IR.
        let temp = Self::new(ir, base_block_size, max_tier_block_size);
        let mut data = Vec::new();
        data.push(temp.tiers.len() as f32);
        data.push(base_block_size as f32);
        for tier in &temp.tiers {
            data.push(tier.tier_block_size as f32);
            data.push(tier.convolver.ir.num_partitions() as f32);
            data.push(tier.ratio as f32);
            for part in &tier.convolver.ir.partitions {
                data.extend_from_slice(part);
            }
        }
        data
    }

    /// Reconstruct a `NonUniformConvolver` from pre-FFT'd data produced by
    /// [`serialize_pre_fft`](Self::serialize_pre_fft).
    ///
    /// # Panics
    ///
    /// Panics if `data` does not contain a valid serialized convolver.
    pub fn from_pre_fft(data: &[f32]) -> Self {
        let tier_count = data[0] as usize;
        let base_block_size = data[1] as usize;
        let mut offset = 2;
        let mut tiers = Vec::with_capacity(tier_count);

        for _ in 0..tier_count {
            let tier_block_size = data[offset] as usize;
            let partition_count = data[offset + 1] as usize;
            let ratio = data[offset + 2] as usize;
            offset += 3;

            let fft_size = 2 * tier_block_size;
            let mut partitions = Vec::with_capacity(partition_count);
            for _ in 0..partition_count {
                let part = data[offset..offset + fft_size].to_vec().into_boxed_slice();
                partitions.push(part);
                offset += fft_size;
            }

            let ir_parts = IrPartitions::from_packed(partitions, tier_block_size);
            let convolver = PartitionedConvolver::new(ir_parts);

            tiers.push(ConvolutionTier {
                convolver,
                tier_block_size,
                ratio,
                input_ring: vec![0.0f32; tier_block_size].into_boxed_slice(),
                input_pos: 0,
                output_ring: vec![0.0f32; tier_block_size].into_boxed_slice(),
                output_read_pos: 0,
            });
        }

        let max_tier = tiers.iter().map(|t| t.tier_block_size).max().unwrap_or(base_block_size);
        Self {
            tiers,
            base_block_size,
            tier_output_scratch: vec![0.0f32; max_tier].into_boxed_slice(),
        }
    }

    /// Clear all internal state. The next block processes as if freshly constructed.
    pub fn reset(&mut self) {
        for tier in &mut self.tiers {
            tier.reset();
        }
    }

    /// The base block size this convolver expects.
    pub fn block_size(&self) -> usize {
        self.base_block_size
    }
}
