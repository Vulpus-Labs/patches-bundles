---
id: "0006"
title: BridgedT primitive — SVF wrapper with soft-clipped feedback
priority: high
created: 2026-05-18
epic: E002
---

## Summary

Build the foundational struck-resonator primitive that the three new
modules in this epic share. It is a **topology-preserving (TPT / ZDF)
state-variable filter**, vendored crate-local, with a soft-clipper on
the bp output. Exposes both the bp tap (resonator output) and the lp
tap (for the voice-level self-FM feedback loop). Pitch droop is the
voice's responsibility — the voice tracks `lp` between ticks and
drives the per-sample `set_f` modulation; see
[ADR 0002 §"Nonlinearity and pitch droop"](../../adrs/0002-bridged-t-resonator-family.md#nonlinearity-and-pitch-droop).

**Substrate change from earlier draft**: previously specified as a
wrapper over `patches_dsp::SvfKernel` (Chamberlin). Empirical testing
shows Chamberlin is wrong for this voice — per-sample `fc`
modulation introduces coefficient errors that detune the resonance,
and the stability margin shrinks at high Q. TPT SVF (Zavalishin) has
trapezoidal integrators with implicit zero-delay feedback, is
unconditionally stable in normalised form, and tracks `fc` accurately
under arbitrary modulation. Mutable Instruments Plaits uses TPT for
the same reasons.

## Acceptance criteria

- [ ] New module `patches-drums/src/primitives/tpt_svf.rs` exposes:
  - [ ] `pub(crate) struct TptSvf` holding sample-rate, coefficients
    (`g`, `k`, `a1`, `a2`, `a3`), and trapezoidal state (`ic1eq`,
    `ic2eq`).
  - [ ] `TptSvf::new(sample_rate: f32) -> Self` zero-initialised.
  - [ ] `TptSvf::set_f_q(&mut self, fc_hz: f32, q: f32)` recomputes
    `g = tan(π·fc/sr)` and the derived coefficients. `q` is classical
    Q (≈ `1/k`); the implementation maps to `k = 1/q.max(0.5)` to
    keep the denominator finite.
  - [ ] `TptSvf::reset_state(&mut self)` clears the trapezoidal state.
  - [ ] `TptSvf::tick(&mut self, x: f32) -> (f32, f32, f32)` returns
    `(lp, hp, bp)` per the standard Zavalishin ZDF recurrence:

    ```text
    v3 = x - ic2eq
    bp = a1*ic1eq + a2*v3
    lp = ic2eq + a2*ic1eq + a3*v3
    ic1eq = 2*bp - ic1eq
    ic2eq = 2*lp - ic2eq
    hp = x - k*bp - lp
    ```

- [ ] New module `patches-drums/src/primitives/bridged_t.rs` exposes:
  - [ ] `pub struct BridgedT` holding a `TptSvf`, `sample_rate`,
    `tune_base`, `q`, `clip`, and the last-tick `lp` reading.
  - [ ] `BridgedT::new(sample_rate: f32, tune_hz: f32, q: f32) -> Self`
    constructs at the given centre frequency and Q.
  - [ ] `BridgedT::set_tune(&mut self, tune_hz: f32)`,
    `BridgedT::set_q(&mut self, q: f32)`,
    `BridgedT::set_clip(&mut self, clip: f32)` — store the new base
    value; coefficients are recomputed each `tick` from the current
    base + FM offset.
  - [ ] `BridgedT::reset_state(&mut self)` clears the TPT state and
    cached `lp`.
  - [ ] `BridgedT::tick(&mut self, x: f32, fm_offset: f32) -> f32`
    runs one sample: clamps `fc = (tune_base * (1 + fm_offset))` so
    `g = tan(π·fc/sr)` stays under `0.4` (mirroring Plaits's
    `CONSTRAIN(f, 0.0f, 0.4f)`), calls `TptSvf::set_f_q`, ticks,
    caches `lp`, applies output `saturate(bp, clip)` (post-tap, not
    in the feedback path — see ADR 0002), returns the saturated bp.
    `fm_offset = 0.0` and `clip = 0.0` together must give the
    bit-identical linear-SVF response.
  - [ ] `BridgedT::lp(&self) -> f32` returns the cached lp from the
    last tick — the voice's self-FM feedback source.
- [ ] `pub use bridged_t::BridgedT` added to
  [primitives/mod.rs](../../src/primitives/mod.rs); `tpt_svf` is
  `pub(crate)` and not re-exported.
- [ ] Unit tests in `tpt_svf.rs`:
  - [ ] **Linear ring**: a unit impulse into a `set_f_q(200, 50)`
    SVF produces output whose dominant FFT bin (4096-sample window)
    is within ±2 bins of 200 Hz at 44.1 kHz.
  - [ ] **Decay coupling**: ring length grows with Q. Compare RMS at
    sample 4000 between `q = 20` and `q = 60`.
  - [ ] **High-Q stability**: 1 s of audio-rate `set_f_q` updates
    (random fc in `[100 Hz, 5 kHz]`, q = 60) with a held impulse on
    the input produces finite, bounded output (no NaN, no
    runaway).
  - [ ] **State sanity**: 10 s of zero input from a fresh instance
    produces all-zero output.
- [ ] Unit tests in `bridged_t.rs`:
  - [ ] **Bypass equivalence**: with `clip = 0`, `fm_offset = 0`,
    a unit impulse produces output equal to a direct `TptSvf` tick
    sequence sample-by-sample.
  - [ ] **Linear ring** at `tune = 100 Hz, q = 50`: dominant FFT
    bin within ±3 bins of 100 Hz.
  - [ ] **Decay coupling**: higher Q → louder at sample 4000.
  - [ ] **FM offset shifts frequency**: with `fm_offset = 0.5`
    held constant for 4096 samples, the dominant bin tracks
    `tune_base * 1.5` within ±3 bins.
  - [ ] **Saturator engages**: with `clip > 0`, third-harmonic
    energy of `ω₀` is measurably larger than at `clip = 0` (the
    output saturator's signature).
  - [ ] **State sanity**: 10 s of zero input + zero fm_offset
    produces all-zero output.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] `cargo test -p patches-drums --lib primitives` green.

## Notes

- The pitch-droop mechanism is **not** in `BridgedT`. The droop is
  driven by the voice (Kick2/Tom2) computing `fm_offset` per sample
  from `BridgedT::lp()` and an attack-FM pulse, then passing it to
  `tick`. This matches the Plaits split: the resonator is generic,
  the FM logic is voice-specific. See
  [ticket 0008](./0008-kick2-module.md) for the voice-side wiring.
- The saturator sits on the bp **output**, not in the feedback path.
  TPT SVF's implicit-equation resolution does not tolerate a
  nonlinearity inside the integrator loop without a Newton solve;
  post-output saturation gives the harmonic dirt without that
  complication, and we no longer need it for droop (which is now
  driven by the FM path).
- The TPT SVF recurrence is in Zavalishin's "The Art of VA Filter
  Design", Ch. 5 §5.6.2. Reference implementation:
  [Plaits stmlib `Svf`](https://github.com/pichenettes/stmlib/blob/master/dsp/filter.h).
- Patches-dsp inclusion is gated on a second consumer needing TPT.
  For now this primitive stays crate-local.
- `saturate` is a `tanh`-flavoured shaper, parameter in `[0, 1]`;
  clip = 0 must be bit-identical to bypass.
