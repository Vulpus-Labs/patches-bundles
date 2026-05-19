---
id: "0012"
title: ModalBank primitive — parallel BridgedT bank with per-partial Q
priority: high
created: 2026-05-19
epic: E003
---

## Summary

Build the resonator-bank primitive that the three modal metal voices
share. Six `BridgedT` instances at configurable inharmonic ratios of
a base frequency, each with its own Q and output gain, sharing one
excitation input. Sum of bandpass taps is the generator output.

Per-partial Q is the entire point: in a real metal, high partials
radiate energy faster than low ones, so the tail darkens. A single
outer envelope can't reproduce this — partials need to decay
inhomogeneously, which means each one needs its own resonator.

See [E003](../../epics/E003-modal-metal-voices.md) and
[ADR 0002](../../adrs/0002-bridged-t-resonator-family.md) §"Linear
core" and §"Excitation shape".

## Acceptance criteria

- [ ] New module `patches-drums/src/primitives/modal_bank.rs`
  exposes:
  - [ ] `pub struct ModalBank` holding six `BridgedT` instances, six
    inharmonic ratios (`[f32; 6]`), six per-partial gains
    (`[f32; 6]`), and the cached base frequency.
  - [ ] `ModalBank::new(sample_rate: f32, base_hz: f32, ratios: [f32; 6], q_profile: [f32; 6], gains: [f32; 6]) -> Self`.
  - [ ] Convenience constructor
    `ModalBank::with_default_metal_profile(sample_rate: f32, base_hz: f32) -> Self`
    using the existing six 808-style ratios from
    [primitives/metallic.rs:2](../../src/primitives/metallic.rs#L2)
    (`[1.0, 1.4471, 1.6170, 1.9265, 2.5028, 2.6637]`), a default
    decreasing Q profile (e.g. `[60.0, 50.0, 45.0, 40.0, 35.0, 30.0]`),
    and unity gains.
  - [ ] `set_base_freq(&mut self, hz: f32)` — recomputes each
    partial's tune as `hz * ratios[i]`.
  - [ ] `set_ratios(&mut self, ratios: [f32; 6])`,
    `set_q_profile(&mut self, qs: [f32; 6])`,
    `set_gains(&mut self, gains: [f32; 6])`.
  - [ ] `set_clip(&mut self, clip: f32)` — broadcasts to all
    `BridgedT` outputs.
  - [ ] `reset_state(&mut self)` — clears all resonator states.
  - [ ] `tick(&mut self, excitation: f32) -> f32` — feeds the same
    excitation sample to every resonator, returns the
    gain-weighted sum of bandpass taps. Internal normalisation
    keeps the typical output amplitude near `±1` when partial gains
    sum to ≈ 1.
  - [ ] `tick_with_modulation(&mut self, excitation: f32, mod_depth_hz: f32, mod_phase: f32) -> f32`
    — same as `tick`, but adds a per-partial frequency offset
    `fast_sine(mod_phase) * mod_depth_hz * ratios[i]` before each
    resonator's tick. Mirrors the existing
    [`MetallicTone::tick_with_modulation`](../../src/primitives/metallic.rs#L57)
    contract so cymbal shimmer can route through it unchanged.
- [ ] `pub use modal_bank::ModalBank` added to
  [primitives/mod.rs](../../src/primitives/mod.rs).
- [ ] Unit tests in `modal_bank.rs`:
  - [ ] **Tail darkens**: with default metal profile and
    `base_hz = 400`, the ratio of HF (4–10 kHz) to total energy
    is lower in samples 4096–8192 than in samples 0–2048. Use
    `band_energy` from [test_support.rs](../../src/test_support.rs).
  - [ ] **Pitch tracking**: `set_base_freq(800)` puts the
    dominant FFT bin near `800 Hz × 1.0` for the fundamental
    partial — assert within ±3 bins at 4096 FFT @ 44.1 kHz.
  - [ ] **Per-partial Q matters**: with a uniform Q profile
    (`[40; 6]`) vs the decreasing default, the HF-darkening test
    above fails for the uniform profile and passes for the
    decreasing one. Sanity check that the profile is doing what
    it claims.
  - [ ] **Reset silences**: after `reset_state` with no
    re-excitation, output is bit-zero for 1 s.
  - [ ] **Modulation engages**: with `mod_depth_hz = 20.0` and a
    slow phase sweep, output samples differ measurably from the
    unmodulated version.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] `just inner -p patches-drums` green.

## Notes

- The bank's six resonators sharing one excitation input means a
  single impulse strikes all partials simultaneously, which is the
  physically correct excitation for a struck metal — energy enters
  the body once, partials extract their share at their own decay
  rates.
- The per-partial gain array allows shaping the spectral envelope
  without changing the Q profile. Default unity gains plus
  internal `/6.0` normalisation mirror `MetallicTone`'s amplitude
  convention. Modules can override per-partial gains to taste —
  e.g. roll off the top two partials slightly for a darker
  cymbal.
- `BridgedT`'s `fm_offset` argument is used by
  `tick_with_modulation`. The base resonator tune stays at
  `base_freq * ratios[i]`; the per-partial FM offset is computed as
  `(mod_depth_hz / base_freq) * sin(mod_phase) * ratios[i]` so the
  modulation is shape-consistent with the existing `MetallicTone`
  shimmer.
- CPU note: six TPT-SVF + saturator per sample, × three voices
  (cymbal + closed hat + open hat) = 18 SVFs in worst-case full
  patch. Measured baseline before this lands so we have a
  comparison number for the post-implementation profile.
- Stability: at very high partial Qs (≥ 70) and large `clip`
  amounts, individual `BridgedT`s can self-oscillate. Default Q
  profile maxes at 60 to stay clear of this.
