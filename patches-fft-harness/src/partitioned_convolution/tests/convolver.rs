use super::*;

#[test]
fn identity_convolution() {
    // Convolving with [1, 0, 0, ...] should reproduce the input.
    let block_size = 16;
    let mut ir = vec![0.0f32; block_size];
    ir[0] = 1.0;
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    let input: Vec<f32> = (0..block_size).map(|i| (i as f32) * 0.1).collect();
    let mut output = vec![0.0f32; block_size];

    // First block: input history is [zeros | input], output should be the input.
    conv.process_block(&input, &mut output);
    // Measured ~0 error on aarch64 macOS debug (2026-04-02). Tightened from 1e-3.
    for i in 0..block_size {
        assert!(
            (output[i] - input[i]).abs() < 1e-5,
            "sample {i}: got {} expected {}",
            output[i],
            input[i],
        );
    }
}

#[test]
fn delayed_impulse_convolution() {
    // IR = [0, 0, ..., 0, 1] with the 1 at position `delay`.
    let block_size = 16;
    let delay = 5;
    let mut ir = vec![0.0f32; block_size];
    ir[delay] = 1.0;
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    // Send several blocks and concatenate output.
    let num_blocks = 4;
    let signal: Vec<f32> = (0..block_size * num_blocks)
        .map(|i| if i < block_size { i as f32 + 1.0 } else { 0.0 })
        .collect();
    let mut output = vec![0.0f32; block_size * num_blocks];

    for b in 0..num_blocks {
        let s = b * block_size;
        conv.process_block(&signal[s..s + block_size], &mut output[s..s + block_size]);
    }

    // Output should be the input delayed by `delay` samples.
    let expected = naive_convolve(&signal, &ir);
    for i in 0..output.len() {
        assert!(
            (output[i] - expected[i]).abs() < 1e-2,
            "sample {i}: got {} expected {}",
            output[i],
            expected[i],
        );
    }
}

#[test]
fn multi_partition_matches_naive() {
    // IR spans 3 partitions.
    let block_size = 16;
    let ir: Vec<f32> = (0..block_size * 3)
        .map(|i| 1.0 / (1.0 + i as f32))
        .collect();
    let parts = IrPartitions::from_ir(&ir, block_size);
    assert_eq!(parts.num_partitions(), 3);
    let mut conv = PartitionedConvolver::new(parts);

    let num_blocks = 8;
    let signal: Vec<f32> = (0..block_size * num_blocks)
        .map(|i| ((i as f32) * 0.37).sin())
        .collect();
    let mut output = vec![0.0f32; signal.len()];

    for b in 0..num_blocks {
        let s = b * block_size;
        conv.process_block(&signal[s..s + block_size], &mut output[s..s + block_size]);
    }

    let expected = naive_convolve(&signal, &ir);
    // Compare only the first signal.len() samples (the tail extends beyond).
    let mut max_err = 0.0f32;
    for i in 0..output.len() {
        let err = (output[i] - expected[i]).abs();
        if err > max_err {
            max_err = err;
        }
    }
    // Measured 1e-6 on aarch64 macOS debug (2026-04-02). Tightened from 0.05.
    assert!(
        max_err < 1e-4,
        "max error {max_err} exceeds tolerance; multi-partition output diverges from naive",
    );
}

#[test]
fn block_boundary_continuity() {
    // Process a continuous signal and check there are no discontinuities
    // at block boundaries.
    let block_size = 32;
    let ir: Vec<f32> = (0..64).map(|i| (-0.01 * i as f32).exp()).collect();
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    let num_blocks = 10;
    let signal: Vec<f32> = (0..block_size * num_blocks)
        .map(|i| ((i as f32) * 0.1).sin())
        .collect();
    let mut output = vec![0.0f32; signal.len()];

    for b in 0..num_blocks {
        let s = b * block_size;
        conv.process_block(&signal[s..s + block_size], &mut output[s..s + block_size]);
    }

    // Check continuity: difference between adjacent samples should be small.
    // For a smooth input convolved with a smooth IR, max sample-to-sample
    // difference should be bounded.
    let expected = naive_convolve(&signal, &ir);
    for b in 1..num_blocks {
        let boundary = b * block_size;
        let err = (output[boundary] - expected[boundary]).abs();
        assert!(
            err < 0.05,
            "discontinuity at block boundary {boundary}: got {} expected {}, err {err}",
            output[boundary],
            expected[boundary],
        );
    }
}

#[test]
fn reset_clears_state() {
    let block_size = 16;
    let ir: Vec<f32> = (0..32).map(|i| 1.0 / (1.0 + i as f32)).collect();
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    let input: Vec<f32> = (0..block_size).map(|i| (i as f32) * 0.1).collect();
    let mut out1 = vec![0.0f32; block_size];
    let mut out2 = vec![0.0f32; block_size];

    // Process one block.
    conv.process_block(&input, &mut out1);
    // Reset and process the same block again.
    conv.reset();
    conv.process_block(&input, &mut out2);

    for i in 0..block_size {
        assert!(
            (out1[i] - out2[i]).abs() < 1e-6,
            "sample {i}: first pass {} != second pass after reset {}",
            out1[i],
            out2[i],
        );
    }
}

#[test]
fn single_sample_ir() {
    // Edge case: IR is a single sample (scalar multiplication).
    let block_size = 8;
    let ir = vec![0.5f32];
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    let input = vec![2.0f32; block_size];
    let mut output = vec![0.0f32; block_size];
    conv.process_block(&input, &mut output);

    for (i, &v) in output.iter().enumerate().take(block_size) {
        assert!(
            (v - 1.0).abs() < 1e-3,
            "sample {i}: got {v} expected 1.0",
        );
    }
}

// ── T-0240: Latency test ────────────────────────────────────────────────

/// Verify that the first non-zero output appears at the expected sample offset.
/// With an identity IR [1, 0, 0, ...], the convolver has zero algorithmic
/// latency — the first output sample should be non-zero when the first
/// input sample is non-zero.
#[test]
fn latency_first_nonzero_output() {
    let block_size = 16;
    let mut ir = vec![0.0f32; block_size];
    ir[0] = 1.0;
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    let mut input = vec![0.0f32; block_size];
    input[0] = 1.0; // impulse at sample 0
    let mut output = vec![0.0f32; block_size];

    conv.process_block(&input, &mut output);

    // First non-zero output should be at sample 0 (no added latency).
    assert!(
        output[0].abs() > 0.5,
        "expected non-zero output at sample 0, got {}",
        output[0]
    );
}

/// With a delayed IR [0, ..., 0, 1] (delay = d), the first non-zero output
/// should appear at sample d.
#[test]
fn latency_delayed_ir_offset() {
    let block_size = 16;
    let delay = 7;
    let mut ir = vec![0.0f32; block_size];
    ir[delay] = 1.0;
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    // Send an impulse at sample 0 across multiple blocks.
    let num_blocks = 4;
    let mut all_output = Vec::new();
    for b in 0..num_blocks {
        let mut input = vec![0.0f32; block_size];
        if b == 0 {
            input[0] = 1.0;
        }
        let mut output = vec![0.0f32; block_size];
        conv.process_block(&input, &mut output);
        all_output.extend_from_slice(&output);
    }

    // Samples before `delay` should be zero.
    for (i, &v) in all_output.iter().enumerate().take(delay) {
        assert!(
            v.abs() < 1e-6,
            "expected zero at sample {i}, got {v}"
        );
    }
    // Sample at `delay` should be non-zero.
    assert!(
        all_output[delay].abs() > 0.5,
        "expected non-zero at sample {delay}, got {}",
        all_output[delay]
    );
}

// ── Exact latency assertions (T-0261) ─────────────────────────────────

/// PartitionedConvolver with identity IR: first output sample equals first
/// input sample (zero algorithmic latency).
#[test]
fn partitioned_exact_latency_identity_ir() {
    let block_size = 32;
    let mut ir = vec![0.0f32; block_size];
    ir[0] = 1.0;
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    let input: Vec<f32> = (0..block_size).map(|i| (i as f32 + 1.0) * 0.1).collect();
    let mut output = vec![0.0f32; block_size];
    conv.process_block(&input, &mut output);

    // Each output sample should match the corresponding input sample
    for i in 0..block_size {
        assert!(
            (output[i] - input[i]).abs() < 1e-3,
            "sample {i}: expected {}, got {} (identity IR should have zero latency)",
            input[i], output[i]
        );
    }
}

/// PartitionedConvolver with delayed impulse IR: first non-zero output
/// appears at exactly sample index D.
#[test]
fn partitioned_exact_latency_delayed_impulse() {
    let block_size = 32;
    let delay = 11;
    let mut ir = vec![0.0f32; block_size];
    ir[delay] = 1.0;
    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut conv = PartitionedConvolver::new(parts);

    // Send an impulse at sample 0
    let num_blocks = 4;
    let mut all_output = Vec::new();
    for b in 0..num_blocks {
        let mut input = vec![0.0f32; block_size];
        if b == 0 { input[0] = 1.0; }
        let mut output = vec![0.0f32; block_size];
        conv.process_block(&input, &mut output);
        all_output.extend_from_slice(&output);
    }

    // Samples 0..delay must be zero
    for (i, &v) in all_output.iter().enumerate().take(delay) {
        assert!(
            v.abs() < 1e-6,
            "sample {i}: expected silence before delay, got {v}"
        );
    }
    // Sample at exactly `delay` must be the impulse
    assert!(
        (all_output[delay] - 1.0).abs() < 1e-3,
        "sample {delay}: expected 1.0, got {} (delayed impulse should appear at exact offset)",
        all_output[delay]
    );
    // Sample after delay should be zero again
    if delay + 1 < all_output.len() {
        assert!(
            all_output[delay + 1].abs() < 1e-3,
            "sample {}: expected silence after impulse, got {}",
            delay + 1, all_output[delay + 1]
        );
    }
}

// --- Direct-convolution reference cross-check (E092 / ticket 0545) ---

/// Cross-check `PartitionedConvolver` against an f64 direct convolution.
///
/// Covers a 64-tap IR (4 partitions at block_size=16) driven by an input at
/// least 3× the partition length (so the input crosses multiple partition
/// boundaries) and asserts the output matches direct convolution sample-for-
/// sample within an FFT-precision tolerance.
///
/// # Latency invariant
///
/// `PartitionedConvolver` has zero algorithmic latency: block `b`'s output
/// covers input samples `[b·N, (b+1)·N)`, matching naive convolution position-
/// for-position. Since `latency == 0`, the "no nonzero samples in positions
/// `0..latency`" check is vacuously satisfied; the per-sample error check
/// already enforces that no output sample arrives earlier than the reference.
#[test]
fn partitioned_convolver_matches_direct_reference() {
    let block_size = 16;
    let ir = reference_ir();
    assert_eq!(ir.len(), 64);
    let parts = IrPartitions::from_ir(&ir, block_size);
    assert_eq!(parts.num_partitions(), 4);
    let mut conv = PartitionedConvolver::new(parts);

    let num_blocks = 12; // 192 samples = 12 × partition length
    let total = num_blocks * block_size;
    assert!(total >= 3 * block_size);

    // Deterministic broadband-ish input: two overlaid sinusoids at
    // incommensurate rates. The exact choice does not matter; what matters
    // is that partition boundaries fall at distinct input phases.
    let signal: Vec<f32> = (0..total)
        .map(|i| {
            let x = i as f32;
            0.8 * (x * 0.37).sin() + 0.3 * (x * 0.11 + 0.7).cos()
        })
        .collect();

    let mut output = vec![0.0_f32; total];
    for b in 0..num_blocks {
        let s = b * block_size;
        conv.process_block(&signal[s..s + block_size], &mut output[s..s + block_size]);
    }

    let reference = naive_convolve_f64(&signal, &ir);

    // Tolerance chosen to reflect accumulated f32 FFT error over 12 blocks
    // with 4 partitions; measured ~1e-5 on aarch64 macOS, 1e-4 gives comfortable
    // margin without masking a real regression.
    const TOLERANCE: f64 = 1.0e-4;
    let mut max_err = 0.0_f64;
    let mut worst_idx = 0usize;
    for i in 0..output.len() {
        let err = ((output[i] as f64) - reference[i]).abs();
        if err > max_err {
            max_err = err;
            worst_idx = i;
        }
    }
    assert!(
        max_err < TOLERANCE,
        "PartitionedConvolver max abs error {max_err:.3e} at sample {worst_idx} \
         exceeds tolerance {TOLERANCE:.0e}"
    );
}
