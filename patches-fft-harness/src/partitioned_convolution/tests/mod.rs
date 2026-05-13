//! Tests for partitioned convolution. Split by subject from the original
//! `tests.rs` per ticket 0538. Shared helpers live here; behaviour-specific
//! tests live in sibling submodules aligned with the impl layout:
//! `complex`, `ir_partitions`, `convolver`, `non_uniform`.

#![allow(unused_imports)]

pub(super) use super::*;
pub(super) use patches_dsp::fft::RealPackedFft;

/// Naive time-domain convolution for reference.
pub(super) fn naive_convolve(signal: &[f32], ir: &[f32]) -> Vec<f32> {
    let out_len = signal.len() + ir.len() - 1;
    let mut out = vec![0.0f32; out_len];
    for (i, &s) in signal.iter().enumerate() {
        for (j, &h) in ir.iter().enumerate() {
            out[i + j] += s * h;
        }
    }
    out
}

/// f64 reference direct-convolution. Drives the candidate-vs-reference error
/// check below. f64 is used so accumulated rounding in the reference does not
/// mask algorithmic error in the f32 candidate.
pub(super) fn naive_convolve_f64(signal: &[f32], ir: &[f32]) -> Vec<f64> {
    let out_len = signal.len() + ir.len() - 1;
    let mut out = vec![0.0_f64; out_len];
    for (i, &s) in signal.iter().enumerate() {
        let sf = s as f64;
        for (j, &h) in ir.iter().enumerate() {
            out[i + j] += sf * h as f64;
        }
    }
    out
}

/// Documented IR for the reference tests: a pair of impulses (at 0 and 7) with
/// a first-order exponentially-decaying tail. The tail exercises sustained
/// multi-partition contributions; the double impulses make partition-boundary
/// errors easy to read off.
pub(super) fn reference_ir() -> Vec<f32> {
    let mut ir = vec![0.0_f32; 64];
    ir[0] = 1.0;
    ir[7] = -0.6;
    for (i, v) in ir.iter_mut().enumerate().skip(1) {
        *v += 0.9_f32 * (-0.05 * i as f32).exp() * ((i as f32) * 0.3).sin();
    }
    ir
}

mod complex;
mod convolver;
mod ir_partitions;
mod non_uniform;
