use patches_dsp::fft::RealPackedFft;

/// Pre-FFT'd impulse response partitions.
pub struct IrPartitions {
    /// Each partition is a 2N-length packed spectrum.
    pub(super) partitions: Vec<Box<[f32]>>,
    pub(super) block_size: usize,
    pub(super) fft_size: usize,
    pub(super) num_partitions: usize,
}

impl IrPartitions {
    /// Partition an impulse response and pre-compute the FFT of each partition.
    ///
    /// `block_size` must be a power of 2 and >= 2 (so that `2 * block_size >= 4`
    /// for `RealPackedFft`). The IR is zero-padded to a multiple of `block_size`.
    ///
    /// # Panics
    ///
    /// Panics if `block_size` is not a power of 2 or is less than 2, or if `ir`
    /// is empty.
    pub fn from_ir(ir: &[f32], block_size: usize) -> Self {
        assert!(block_size >= 2 && block_size.is_power_of_two());
        assert!(!ir.is_empty());

        let fft_size = 2 * block_size;
        let fft = RealPackedFft::new(fft_size);
        let num_partitions = ir.len().div_ceil(block_size);

        let mut partitions = Vec::with_capacity(num_partitions);
        for i in 0..num_partitions {
            let start = i * block_size;
            let end = (start + block_size).min(ir.len());
            let mut buf = vec![0.0f32; fft_size];
            buf[..end - start].copy_from_slice(&ir[start..end]);
            // Remaining samples are already zero (zero-pad to 2N).
            fft.forward(&mut buf);
            partitions.push(buf.into_boxed_slice());
        }

        Self {
            partitions,
            block_size,
            fft_size,
            num_partitions,
        }
    }

    /// Create `IrPartitions` from pre-computed frequency-domain partition data.
    ///
    /// `partitions` must contain frequency-domain spectra of length `2 * block_size`
    /// in CMSIS packed format (as produced by [`RealPackedFft::forward`]).
    ///
    /// This skips the forward FFT step, allowing the caller to pre-compute
    /// spectral data (e.g. during file processing on the control thread).
    pub fn from_packed(partitions: Vec<Box<[f32]>>, block_size: usize) -> Self {
        let fft_size = 2 * block_size;
        let num_partitions = partitions.len();
        Self { partitions, block_size, fft_size, num_partitions }
    }

    /// Number of IR partitions.
    pub fn num_partitions(&self) -> usize {
        self.num_partitions
    }

    /// Block size (N).
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// FFT size (2N).
    pub fn fft_size(&self) -> usize {
        self.fft_size
    }
}
