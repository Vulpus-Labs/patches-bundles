use super::*;

#[test]
fn nu_identity_convolution() {
    let block_size = 16;
    let mut ir = vec![0.0f32; block_size];
    ir[0] = 1.0;
    let mut conv = NonUniformConvolver::new(&ir, block_size, block_size);

    let input: Vec<f32> = (0..block_size).map(|i| (i as f32) * 0.1).collect();
    let mut output = vec![0.0f32; block_size];

    conv.process_block(&input, &mut output);
    for i in 0..block_size {
        assert!(
            (output[i] - input[i]).abs() < 1e-3,
            "sample {i}: got {} expected {}",
            output[i],
            input[i],
        );
    }
}

#[test]
fn nu_delayed_impulse() {
    let block_size = 16;
    let delay = 5;
    let mut ir = vec![0.0f32; block_size];
    ir[delay] = 1.0;
    let mut conv = NonUniformConvolver::new(&ir, block_size, block_size);

    let num_blocks = 4;
    let signal: Vec<f32> = (0..block_size * num_blocks)
        .map(|i| if i < block_size { i as f32 + 1.0 } else { 0.0 })
        .collect();
    let mut output = vec![0.0f32; block_size * num_blocks];

    for b in 0..num_blocks {
        let s = b * block_size;
        conv.process_block(&signal[s..s + block_size], &mut output[s..s + block_size]);
    }

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
fn nu_multi_tier_matches_naive() {
    // IR long enough to span multiple tiers (base=16, max=64).
    // Tier 0: block=16, covers IR[0..16]
    // Tier 1: block=32, covers IR[16..48]
    // Tier 2: block=64, covers IR[48..end]
    let block_size = 16;
    let ir: Vec<f32> = (0..200).map(|i| 1.0 / (1.0 + i as f32)).collect();
    let mut conv = NonUniformConvolver::new(&ir, block_size, 64);

    let num_blocks = 30;
    let signal: Vec<f32> = (0..block_size * num_blocks)
        .map(|i| ((i as f32) * 0.37).sin())
        .collect();
    let mut output = vec![0.0f32; signal.len()];

    for b in 0..num_blocks {
        let s = b * block_size;
        conv.process_block(&signal[s..s + block_size], &mut output[s..s + block_size]);
    }

    let expected = naive_convolve(&signal, &ir);
    let mut max_err = 0.0f32;
    for i in 0..output.len() {
        let err = (output[i] - expected[i]).abs();
        if err > max_err {
            max_err = err;
        }
    }
    assert!(
        max_err < 0.1,
        "max error {max_err} exceeds tolerance; non-uniform output diverges from naive",
    );
}

#[test]
fn nu_matches_uniform() {
    // Compare non-uniform against uniform convolver for the same IR.
    let block_size = 16;
    let ir: Vec<f32> = (0..128).map(|i| (-0.02 * i as f32).exp()).collect();

    let parts = IrPartitions::from_ir(&ir, block_size);
    let mut uniform = PartitionedConvolver::new(parts);
    let mut non_uniform = NonUniformConvolver::new(&ir, block_size, 64);

    let num_blocks = 20;
    let signal: Vec<f32> = (0..block_size * num_blocks)
        .map(|i| ((i as f32) * 0.23).sin())
        .collect();
    let mut out_u = vec![0.0f32; signal.len()];
    let mut out_nu = vec![0.0f32; signal.len()];

    for b in 0..num_blocks {
        let s = b * block_size;
        uniform.process_block(&signal[s..s + block_size], &mut out_u[s..s + block_size]);
        non_uniform.process_block(&signal[s..s + block_size], &mut out_nu[s..s + block_size]);
    }

    let mut max_err = 0.0f32;
    for i in 0..out_u.len() {
        let err = (out_u[i] - out_nu[i]).abs();
        if err > max_err {
            max_err = err;
        }
    }
    assert!(
        max_err < 0.05,
        "max error {max_err} between uniform and non-uniform convolver",
    );
}

#[test]
fn nu_reset_clears_state() {
    let block_size = 16;
    let ir: Vec<f32> = (0..64).map(|i| 1.0 / (1.0 + i as f32)).collect();
    let mut conv = NonUniformConvolver::new(&ir, block_size, 32);

    let input: Vec<f32> = (0..block_size).map(|i| (i as f32) * 0.1).collect();
    let mut out1 = vec![0.0f32; block_size];
    let mut out2 = vec![0.0f32; block_size];

    conv.process_block(&input, &mut out1);
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
fn nu_single_sample_ir() {
    let block_size = 8;
    let ir = vec![0.5f32];
    let mut conv = NonUniformConvolver::new(&ir, block_size, 32);

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

#[test]
fn nu_tier_count() {
    // IR of 200 samples, base=16, max=64:
    // Tier 0: block=16 → IR[0..16]
    // Tier 1: block=32 → IR[16..48]
    // Tier 2: block=64 → IR[48..200] (final tier, multiple partitions)
    let ir = vec![1.0f32; 200];
    let conv = NonUniformConvolver::new(&ir, 16, 64);
    assert_eq!(conv.tiers.len(), 3);
    assert_eq!(conv.tiers[0].tier_block_size, 16);
    assert_eq!(conv.tiers[1].tier_block_size, 32);
    assert_eq!(conv.tiers[2].tier_block_size, 64);
}

/// NonUniformConvolver with identity IR: latency equals base_block_size.
#[test]
fn non_uniform_exact_latency_identity_ir() {
    let base_block = 16;
    let mut ir = vec![0.0f32; base_block];
    ir[0] = 1.0;
    let mut conv = NonUniformConvolver::new(&ir, base_block, base_block);

    // Send impulse at sample 0 and collect several blocks
    let num_blocks = 4;
    let mut all_output = Vec::new();
    for b in 0..num_blocks {
        let mut input = vec![0.0f32; base_block];
        if b == 0 { input[0] = 1.0; }
        let mut output = vec![0.0f32; base_block];
        conv.process_block(&input, &mut output);
        all_output.extend_from_slice(&output);
    }

    // Find the first non-zero output sample
    let first_nonzero = all_output.iter().position(|&v| v.abs() > 0.5);
    assert!(
        first_nonzero.is_some(),
        "no non-zero output found in {} samples",
        all_output.len()
    );
    let idx = first_nonzero.unwrap();
    // Document actual latency — for single-tier NonUniform, it should be 0
    // (same as PartitionedConvolver since tier 0 processes immediately)
    assert!(
        idx == 0,
        "NonUniformConvolver identity IR: first non-zero at sample {idx}, expected 0"
    );
}

#[test]
fn nu_long_ir_matches_naive() {
    // Simulate a realistic reverb: 2048-sample IR, base=64, max=512.
    let block_size = 64;
    let ir: Vec<f32> = (0..2048).map(|i| (-0.003 * i as f32).exp() * 0.1).collect();
    let mut conv = NonUniformConvolver::new(&ir, block_size, 512);

    let num_blocks = 60;
    let signal: Vec<f32> = (0..block_size * num_blocks)
        .map(|i| ((i as f32) * 0.11).sin())
        .collect();
    let mut output = vec![0.0f32; signal.len()];

    for b in 0..num_blocks {
        let s = b * block_size;
        conv.process_block(&signal[s..s + block_size], &mut output[s..s + block_size]);
    }

    let expected = naive_convolve(&signal, &ir);
    let mut max_err = 0.0f32;
    for i in 0..output.len() {
        let err = (output[i] - expected[i]).abs();
        if err > max_err {
            max_err = err;
        }
    }
    // Measured 3.2e-4 on aarch64 macOS debug (2026-04-02). Tightened from 0.1.
    assert!(
        max_err < 0.01,
        "max error {max_err} for long IR non-uniform convolution",
    );
}

/// Same cross-check against `NonUniformConvolver`.
///
/// NonUniform splits the IR into geometrically-growing tiers. Tier 0 processes
/// every call at `base_block_size`; larger tiers accumulate input before
/// firing. Internally each tier has its own delay, but the summed output
/// across tiers aligns position-for-position with naive convolution (same
/// latency invariant as `PartitionedConvolver`: zero).
#[test]
fn non_uniform_convolver_matches_direct_reference() {
    let base_block = 16;
    let max_tier_block = 32;
    let ir = reference_ir();
    let mut conv = NonUniformConvolver::new(&ir, base_block, max_tier_block);

    let num_blocks = 16; // 256 samples > 3× IR length
    let total = num_blocks * base_block;
    let signal: Vec<f32> = (0..total)
        .map(|i| {
            let x = i as f32;
            0.6 * (x * 0.29).sin() + 0.4 * (x * 0.17 + 1.1).cos()
        })
        .collect();

    let mut output = vec![0.0_f32; total];
    for b in 0..num_blocks {
        let s = b * base_block;
        conv.process_block(&signal[s..s + base_block], &mut output[s..s + base_block]);
    }

    let reference = naive_convolve_f64(&signal, &ir);

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
        "NonUniformConvolver max abs error {max_err:.3e} at sample {worst_idx} \
         exceeds tolerance {TOLERANCE:.0e}"
    );
}
