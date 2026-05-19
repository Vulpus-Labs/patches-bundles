---
id: "0015"
title: XorPairTone primitive — three XOR-pair squares summed
priority: medium
created: 2026-05-19
epic: E004
---

## Summary

Build the inharmonic generator that the three XOR-flavour metal
voices share. Six square oscillators arranged as three pairs; each
pair's outputs are multiplied (bipolar XOR = signed product), the
three products are summed and normalised.

The texture is denser and coarser than the existing
[`MetallicTone`](../../src/primitives/metallic.rs)'s direct sum of
six independent squares because each pair's product carries energy
at the sum and difference of its two inputs' frequencies, in addition
to the original components beating against each other.

API shape mirrors `MetallicTone` so the XOR-flavour modules differ
from their `MetallicTone`-based counterparts only by a primitive
swap.

See [E004](../../epics/E004-xor-metal-voices.md).

## Acceptance criteria

- [ ] New module `patches-drums/src/primitives/xor_pair_tone.rs`
  exposes:
  - [ ] `pub struct XorPairTone` holding six oscillator phases,
    six per-oscillator phase increments, the cached base frequency,
    sample rate, and `sr_recip` — mirror the existing
    [`MetallicTone`](../../src/primitives/metallic.rs#L6-L11)
    layout.
  - [ ] Six base ratios `XOR_PAIR_RATIOS: [f32; 6]` organised as
    three pairs:
    - Pair A: indices 0, 1 (e.g. `[1.0, 1.4471]`)
    - Pair B: indices 2, 3 (e.g. `[1.6170, 1.9265]`)
    - Pair C: indices 4, 5 (e.g. `[2.5028, 2.6637]`)

    Use the same six values as
    [`METALLIC_RATIOS`](../../src/primitives/metallic.rs#L2) for
    direct A/B comparison against the existing `MetallicTone`. The
    pair-grouping shape (which ratios share a pair) is the design
    decision that gives the XOR flavour its character; pin it as
    a constant rather than a parameter for v1.
  - [ ] `XorPairTone::new(sample_rate: f32) -> Self` with default
    zero phases and zero increments.
  - [ ] `set_frequency(&mut self, base_hz: f32)` — sets each
    oscillator's increment to `base_hz * XOR_PAIR_RATIOS[i] / sr`,
    clamped to ≤ 0.499. Same shape as
    [`MetallicTone::set_frequency`](../../src/primitives/metallic.rs#L24).
  - [ ] `reset(&mut self)` — zeros all six phases.
  - [ ] `trigger(&mut self)` — calls `reset`. Same contract as
    `MetallicTone::trigger`.
  - [ ] `tick(&mut self) -> f32`:
    1. For each of the six oscillators, compute a bipolar square
       value `+1.0` if `phase < 0.5` else `-1.0`.
    2. Three pair products: `p_a = sq[0] * sq[1]`,
       `p_b = sq[2] * sq[3]`, `p_c = sq[4] * sq[5]`.
    3. Sum: `(p_a + p_b + p_c) / 3.0`.
    4. Advance and wrap all six phases.
    Return the sum.
  - [ ] `tick_with_modulation(&mut self, mod_depth_hz: f32, mod_phase: f32) -> f32`
    — same as `tick`, but adds
    `fast_sine(mod_phase) * mod_depth_hz * XOR_PAIR_RATIOS[i] / sr`
    to each oscillator's phase increment for that sample, clamped to
    `[0.0, 0.499]`. Mirror
    [`MetallicTone::tick_with_modulation`](../../src/primitives/metallic.rs#L57).
- [ ] `pub use xor_pair_tone::XorPairTone` added to
  [primitives/mod.rs](../../src/primitives/mod.rs).
- [ ] Unit tests in `xor_pair_tone.rs`:
  - [ ] **Trigger produces output** — after `trigger()` and
    `set_frequency(400.0)`, RMS over 1000 samples > 0.1. Mirror
    [metallic.rs:84–97](../../src/primitives/metallic.rs#L84-L97).
  - [ ] **Output bounded** — every sample in `[-1, 1]`. Mirror
    [metallic.rs:99–112](../../src/primitives/metallic.rs#L99-L112).
  - [ ] **Differs from `MetallicTone`** — run both at
    `base_hz = 600` for 4096 samples and compare spectra: average
    absolute sample difference > 0.05 (the two generators are
    genuinely different).
  - [ ] **Intermod product present** — at `base_hz = 500` with
    pair A = `[1.0, 1.4471]`, FFT shows energy near the
    difference frequency `500 * (1.4471 - 1.0) = 223.55 Hz`
    that does **not** appear in a `MetallicTone` run at the same
    base frequency. This is the primitive's reason for existing;
    pin the behaviour as a regression test.
  - [ ] **Reset zeros phases** — same shape as
    [metallic.rs:114–127](../../src/primitives/metallic.rs#L114-L127).
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] `just inner -p patches-drums` green.

## Notes

- Bipolar XOR is multiplication: `sq_a * sq_b` is `+1` when both
  inputs agree (both high or both low) and `-1` when they
  disagree. The output is itself a square wave whose transitions
  occur at every transition of either input, weighted by sign —
  spectral content is dense in the sum and difference of the two
  inputs' frequencies plus their odd-harmonic combinations.
- The `/3.0` normalisation matches `MetallicTone`'s `/6.0`
  convention adapted for three terms; gives comparable output
  amplitude so module shells can swap generators without
  re-balancing the rest of the mix.
- Pair-grouping (which ratios pair together) is a design choice
  with sonic consequences. Pairs of nearby ratios (e.g.
  `[1.0, 1.4471]`) produce lower difference frequencies and slow
  beating; pairs of distant ratios (e.g. `[1.0, 2.6637]`) produce
  higher difference frequencies and denser high content. v1
  groups adjacent ratios — `[0,1]`, `[2,3]`, `[4,5]` — which gives
  three difference-frequency regions roughly evenly spaced. If a
  later tuning sweep reveals a more characterful grouping, change
  it then.
- `fast_sine` is already imported in
  [primitives/metallic.rs:58](../../src/primitives/metallic.rs#L58)
  via `patches_dsp::fast_sine`. Same import works here.
- CPU note: six phase increments + three multiplies + wrap per
  sample. Cheaper than `ModalBank` and roughly comparable to
  `MetallicTone`.
