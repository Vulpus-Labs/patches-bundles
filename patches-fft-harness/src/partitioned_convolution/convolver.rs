use patches_dsp::fft::RealPackedFft;

use super::complex::{complex_multiply_accumulate_packed, complex_multiply_packed};
use super::ir_partitions::IrPartitions;

/// Uniform partitioned convolver using overlap-save.
///
/// Call `process_block` once per N-sample input block. All internal buffers are
/// pre-allocated; the hot path performs zero heap allocations.
pub struct PartitionedConvolver {
    pub(super) ir: IrPartitions,
    fft: RealPackedFft,
    /// Circular buffer of frequency-domain input spectra.
    fdl: Vec<Box<[f32]>>,
    /// Current write position in the FDL (advances by 1 per block).
    fdl_pos: usize,
    /// Time-domain input history: `[prev_N | current_N]`, length 2N.
    input_history: Box<[f32]>,
    /// Frequency-domain accumulator, length 2N.
    accumulator: Box<[f32]>,
    /// Time-domain scratch for IFFT output, length 2N.
    output_buf: Box<[f32]>,
}

impl PartitionedConvolver {
    /// Create a new convolver for the given pre-partitioned IR.
    pub fn new(ir: IrPartitions) -> Self {
        let fft_size = ir.fft_size();
        let fft = RealPackedFft::new(fft_size);
        let num_partitions = ir.num_partitions();

        let fdl: Vec<Box<[f32]>> = (0..num_partitions)
            .map(|_| vec![0.0f32; fft_size].into_boxed_slice())
            .collect();

        Self {
            ir,
            fft,
            fdl,
            fdl_pos: 0,
            input_history: vec![0.0f32; fft_size].into_boxed_slice(),
            accumulator: vec![0.0f32; fft_size].into_boxed_slice(),
            output_buf: vec![0.0f32; fft_size].into_boxed_slice(),
        }
    }

    /// Process one block of N input samples, writing N output samples.
    ///
    /// # Panics
    ///
    /// Panics if `input.len()` or `output.len()` does not equal `block_size`.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32]) {
        let n = self.ir.block_size();
        let fft_size = self.ir.fft_size();
        assert_eq!(input.len(), n);
        assert_eq!(output.len(), n);

        // 1. Update input history: shift left by N, write new block into right half.
        self.input_history.copy_within(n..fft_size, 0);
        self.input_history[n..].copy_from_slice(input);

        // 2. FFT the 2N input history into the current FDL slot.
        let fdl_slot = &mut self.fdl[self.fdl_pos];
        fdl_slot.copy_from_slice(&self.input_history);
        self.fft.forward(fdl_slot);

        // 3. Accumulate: for each partition i, multiply FDL[(pos - i) mod B] * H[i].
        //    First partition uses non-accumulating multiply (avoids zeroing pass);
        //    remaining partitions accumulate into the result.
        let num_p = self.ir.num_partitions();
        {
            let fdl_idx = self.fdl_pos % num_p;
            complex_multiply_packed(
                &mut self.accumulator,
                &self.fdl[fdl_idx],
                &self.ir.partitions[0],
            );
        }
        for i in 1..num_p {
            let fdl_idx = (self.fdl_pos + num_p - i) % num_p;
            complex_multiply_accumulate_packed(
                &mut self.accumulator,
                &self.fdl[fdl_idx],
                &self.ir.partitions[i],
            );
        }

        // 4. IFFT the accumulator.
        self.output_buf.copy_from_slice(&self.accumulator);
        self.fft.inverse(&mut self.output_buf);

        // 5. Take the last N samples (overlap-save).
        output.copy_from_slice(&self.output_buf[n..]);

        // 6. Advance FDL position.
        self.fdl_pos = (self.fdl_pos + 1) % num_p;
    }

    /// Clear all internal state (FDL, input history). The next block will
    /// process as if the convolver were freshly constructed.
    pub fn reset(&mut self) {
        for slot in &mut self.fdl {
            slot.fill(0.0);
        }
        self.fdl_pos = 0;
        self.input_history.fill(0.0);
        self.accumulator.fill(0.0);
        self.output_buf.fill(0.0);
    }

    /// The block size (N) this convolver expects.
    pub fn block_size(&self) -> usize {
        self.ir.block_size()
    }
}
