---
id: "0010"
title: Claves2 module — cascaded struck resonator with re-excitation
priority: medium
created: 2026-05-18
epic: E002
---

## Summary

Add `Claves2`, a two-stage cascaded struck-resonator clave. Stage 1 is
excited by the trigger pulse and rings at the tuned frequency. Stage
1's bandpass output, passed through a rising-edge detector against a
threshold, becomes the **excitation pulse** for stage 2. Stage 2 rings
at the same nominal tune and is re-excited by every cycle of stage
1's burst, giving an effective Q well past what stage 2's own Q
parameter would sustain — a short, very bright, fast-decaying click
with an extended ringing tail. A `cascade_mix` parameter blends
stage-1-only against stage-1+stage-2, letting the module collapse to a
single-stage equivalent of the existing [Claves](../../src/claves.rs)
at one end of the knob.

Sister to existing [Claves](../../src/claves.rs); the two coexist.

See [ADR 0002](../../adrs/0002-bridged-t-resonator-family.md)
§"Cascading (Claves2)".

## Acceptance criteria

- [ ] New module `patches-drums/src/claves2.rs`.
- [ ] Module name: `Claves2`. ABI version 1. Added to
  `patches_drums::register` and to the `export_modules!` invocation in
  [lib.rs](../../src/lib.rs#L41-L66).
- [ ] Ports (matching `Claves`'s convention):
  - [ ] `trigger` (mono, trigger).
  - [ ] `velocity` (mono, audio) — latched on trigger, defaults to
    1.0 when disconnected.
  - [ ] `out` (mono, audio).
- [ ] Realtime parameters:
  - [ ] `tune` (float, 500–5000 Hz, default 2500) — stage frequency.
  - [ ] `q` (float, 10.0–80.0, default 40) — stage Q. The cascade
    pushes effective Q well above this.
  - [ ] `cascade_mix` (float, 0.0–1.0, default 0.7) — output
    crossfade between stage 1 alone (0.0) and stage 1 + stage 2
    (1.0).
  - [ ] `pulse_ms` (float, 0.05–2.0, default 0.5) — trigger
    excitation duration. Defaults short because the cascade extends
    the audible tail without needing a long initial pulse.
  - [ ] `clip` (float, 0.0–1.0, default 0.2) — feedback-clipper
    amount on both stages.
- [ ] Structural parameter:
  - [ ] `pulse_shape` (enum: `Dirac` | `ExpDecay` | `HalfSine` |
    `FilteredClick`, default `Dirac`) — `Dirac` is a sensible default
    for a clave because the resonator's character carries the sound
    and the trigger pulse should be sharp.
- [ ] Process loop:
  - [ ] Two `BridgedT` instances (stage 1, stage 2) initialised at
    the same tune and Q.
  - [ ] One `Excitation` for stage 1.
  - [ ] On trigger rising edge: latch velocity, reset both
    resonators, `excitation.trigger()`. Reset the re-excitation
    edge detector.
  - [ ] Per sample:
    1. `s1_in = excitation.tick()`.
    2. `s1_out = bridged_t_1.tick(s1_in)`.
    3. Run `s1_out` through a rising-edge detector: a flag goes
       high when `s1_out` crosses an internal threshold upward (e.g.
       0.05 of the peak observed since trigger). When the flag goes
       high, set `s2_in = s1_out` for that sample; otherwise
       `s2_in = 0`.
    4. `s2_out = bridged_t_2.tick(s2_in)`.
    5. `output = lerp(s1_out, s1_out + s2_out, cascade_mix)`.
    6. Scale by latched velocity → write to `out`.
  - [ ] The edge detector is the entire cascade mechanism. Implement
    it carefully: hysteresis (rising-edge only) prevents stage 2
    being driven by every sample of stage 1's positive half-cycle;
    threshold scaling against running peak makes the detector
    self-calibrating across tune/q settings.
- [ ] Module rustdoc table mirrors the `Claves`-style description
  block at the top of [claves.rs:1–25](../../src/claves.rs#L1-L25).
  Document the cascade mechanism in module-level rustdoc — the
  "stage 1 re-excites stage 2" trick is the entire reason this
  module exists and is non-obvious from the parameter list alone.
- [ ] Unit tests in `claves2.rs`:
  - [ ] **Trigger produces output** — RMS > 0.001 over the first 500
    samples.
  - [ ] **Pitch tracking** — dominant FFT bin at `tune = 4000 Hz` is
    higher than at `tune = 1000 Hz`.
  - [ ] **Single-stage collapse** — with `cascade_mix = 0.0`,
    output is bit-identical (within float tolerance) to a
    single-stage equivalent: build a small inline reference using
    one `BridgedT` and one `Excitation` with the same params, run
    both for 500 samples, compare sample-by-sample.
  - [ ] **Cascade extends ring** — with `cascade_mix = 1.0` and
    `q = 30`, RMS at sample 600 is higher than with
    `cascade_mix = 0.0` and the same `q`. (Stage 2's re-excitation
    sustains energy past stage 1's natural decay.)
  - [ ] **Output decays** — at default params, RMS at samples
    4000–4500 < 0.01 (clave is short; the cascade extends but does
    not sustain).
  - [ ] **Velocity scales output** — half velocity ≈ half RMS,
    tolerance ±10%.
- [ ] FFI manifest test in [lib.rs:88](../../src/lib.rs#L88) updated
  to include `"Claves2"` in `EXPECTED_NAMES`.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.

## Notes

- The rising-edge detector is the cascade. Two design choices are
  on the table:
  - **Fixed threshold** (e.g. 0.1): simple, but behaviour depends on
    tune/q because the resonator's peak amplitude varies.
  - **Running-peak threshold** (e.g. 0.05 × peak-since-trigger):
    self-calibrating, slightly more state, recommended.
  Pick running-peak.
- Stage 2's Q matters less than the cascade rate. With aggressive
  cascading the audible decay is driven by how fast stage 1's bp
  output crosses the threshold, not by stage 2's own ring. Document
  this in the module rustdoc.
- DC offset: stage 1's bp tap is DC-free by construction, so the
  edge detector doesn't need to chase a moving baseline. If
  profiling reveals slow drift from the cascade-mix branch, add an
  explicit DC blocker on `s2_in`; not expected to be needed.
- If `Kick2`, `Tom2`, and `Claves2` all share a process-loop spine
  by the time this lands, consider extracting
  `StruckResonatorVoice` as a follow-up. The cascade form here is
  enough of a deviation that the shared spine may only cover
  `Kick2`/`Tom2` — that's fine, two call sites is the bar for
  extraction.
- The existing single-stage [Claves](../../src/claves.rs) module
  remains. `Claves2` is not a replacement.
