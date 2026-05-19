---
id: "0018"
title: TptSvf — use `fast_tan_small` and cache `pi_over_sr`
priority: medium
created: 2026-05-19
---

## Summary

[`TptSvf::set_f_q`](../../src/primitives/tpt_svf.rs#L66) computes
`g = tan(π · fc / sr)` per call, and `set_f_q` is called from
[`BridgedT::tick`](../../src/primitives/bridged_t.rs#L83) every
sample. With six bandpass resonators in [`ModalBank`](../../src/primitives/modal_bank.rs)
plus a body resonator in [`Cymbal2`](../../src/cymbal2.rs), a single
Cymbal2 voice issues 7 × `f32::tan` calls/sample (≈ 310 k tan/sec at
44.1 kHz). `f32::tan` is libm — ~30–50 cycles incl. range reduction.

`patches_dsp::approximate::fast_tan_small(angle, g_max)` (new in
0.6.1) returns the same `g` value to ≤ 2e-4 absolute error on the
operating range `[0, atan(G_MAX)]` for ~3 mul + 2 add. Plug it in,
and also cache `1.0/sample_rate` so the per-sample expression is a
single `fc * pi_over_sr` multiply instead of `PI * fc / sample_rate`.

This is a drop-in arithmetic change. No API surface, no behaviour
change beyond the documented ≤ 0.6-cent pitch error. Complementary
to the larger CoefRamp adoption (ticket 0023); the savings stack —
0018 reduces the per-sample cost of *any* path that calls
`set_f_q`, including the Kick2/Tom2 self-FM path that 0023 leaves
on the per-sample variant.

## Acceptance criteria

- [ ] [`TptSvf`](../../src/primitives/tpt_svf.rs#L39) caches
      `pi_over_sr: f32` in `new`, equal to `PI / sample_rate`.
- [ ] [`TptSvf::set_f_q`](../../src/primitives/tpt_svf.rs#L66)
      computes `g = fast_tan_small(fc * pi_over_sr, G_MAX)` instead
      of `(PI * fc / sample_rate).tan().min(G_MAX)`.
- [ ] All existing `TptSvf` tests pass unmodified.
- [ ] All existing [`BridgedT`](../../src/primitives/bridged_t.rs)
      tests pass unmodified.
- [ ] All existing [`ModalBank`](../../src/primitives/modal_bank.rs),
      [`Cymbal2`](../../src/cymbal2.rs),
      [`ClosedHiHat2` / `OpenHiHat2`](../../src/hihat2.rs),
      [`Kick2`](../../src/kick2.rs), and
      [`Tom2`](../../src/tom2.rs) tests pass unmodified.
- [ ] `cargo test -p patches-drums` green.

## Notes

- `fast_tan_small` saturates at `g_max` once `angle ≥ ANGLE_MAX`
  (≈ atan(0.4)), so the post-clamp `.min(G_MAX)` is folded into the
  approximation and need not be applied again at the call site.
- Accuracy tests for `fast_tan_small` itself live in `patches-dsp`
  (≤ 2e-4 abs error verified there). The drum-side tests are the
  end-to-end ones — frequency-tracking, decay, spectral shape — and
  the existing tolerances cover the polynomial error with plenty of
  margin.
- Worth a perf-style sanity check on a representative session
  (drum_machine2 example) after landing, but no benchmark gate.
