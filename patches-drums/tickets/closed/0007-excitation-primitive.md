---
id: "0007"
title: Excitation primitive — short shaped pulse generator
priority: high
created: 2026-05-18
epic: E002
---

## Summary

Build the excitation-side primitive that strikes the bridged-T
resonator. A single-sample Dirac into a high-Q filter rings at the
right frequency but sounds thin; the analog circuits deliver a short
asymmetric pulse a few milliseconds long, and the pulse shape carries
much of the voice's character (weight, thump, click, snap). This
primitive synthesises that pulse.

See [ADR 0002](../../adrs/0002-bridged-t-resonator-family.md)
§"Excitation shape".

## Acceptance criteria

- [ ] New module `patches-drums/src/primitives/excitation.rs` exposes:
  - [ ] `pub enum PulseShape { Dirac, ExpDecay, HalfSine, FilteredClick }`
    selecting the shape. Variants carry no per-instance data; per-shape
    tunables go on `Excitation` itself so the audio path stays
    branch-free per sample.
  - [ ] `pub struct Excitation` holding `sample_rate`, current
    `shape`, `pulse_ms`, `lp_hz` (used only by `FilteredClick`), an
    internal sample counter, a precomputed length-in-samples, and any
    shape-specific state (one-pole state for the filtered-click
    variant, an `xorshift64` PRNG seed for the same).
  - [ ] `Excitation::new(sample_rate: f32, instance_seed: u64) -> Self`
    with sensible defaults (`Dirac`, `pulse_ms = 2.0`,
    `lp_hz = 4000.0`).
  - [ ] `Excitation::set_shape(&mut self, shape: PulseShape)`,
    `Excitation::set_pulse_ms(&mut self, ms: f32)`,
    `Excitation::set_lp_hz(&mut self, hz: f32)` — each recomputes
    cached state (length-in-samples, one-pole coefficient) as needed.
  - [ ] `Excitation::trigger(&mut self)` — resets the counter and
    primes shape-specific state. Idempotent if called mid-pulse
    (restarts the pulse).
  - [ ] `Excitation::tick(&mut self) -> f32` — emits one sample of
    the active pulse, or `0.0` if the pulse has completed. Branch on
    `shape` outside the per-sample hot loop is fine; per-sample tick
    must not match on shape.
  - [ ] `Excitation::is_active(&self) -> bool` for callers that want
    to skip downstream processing when no excitation is in flight.
- [ ] `pub use excitation::{Excitation, PulseShape}` added to
  [primitives/mod.rs](../../src/primitives/mod.rs).
- [ ] Per-shape behaviour:
  - [ ] `Dirac`: sample 0 emits 1.0, all subsequent samples emit
    0.0 until the next `trigger()`.
  - [ ] `ExpDecay`: amplitude `exp(-t / τ)` where
    `τ = pulse_ms / 1000`. Pulse ends when amplitude < 1e-4 or after
    `5 · τ` samples, whichever first.
  - [ ] `HalfSine`: amplitude `sin(π · t / T)` for
    `t ∈ [0, T]`, `T = pulse_ms / 1000`. Exactly one half-cycle.
    Zero after `T`.
  - [ ] `FilteredClick`: white noise (xorshift64-derived) for
    `T = pulse_ms / 1000` seconds, passed through a one-pole low-pass
    at `lp_hz`. Zero after `T`.
- [ ] Avoid per-sample branching on `PulseShape` in the hot loop.
  Pick the implementation strategy that fits the bundle's existing
  conventions — either dispatch in `tick` once and run a small
  per-shape inline tick, or store a function pointer / inline the
  shape-specific accumulator update. Either is fine; document the
  choice in the file's module-level rustdoc.
- [ ] Unit tests in `excitation.rs`:
  - [ ] **Dirac sample-zero**: after `trigger()`, sample 0 is 1.0,
    samples 1..100 are 0.0.
  - [ ] **Decay monotonicity**: `ExpDecay` with `pulse_ms = 5.0`
    produces a monotonically decreasing sequence after sample 0.
  - [ ] **HalfSine shape**: `HalfSine` with `pulse_ms = 4.0` peaks
    near the middle (`T/2`) and returns to zero at sample
    `T · sample_rate`.
  - [ ] **FilteredClick spectrum**: with `lp_hz = 500.0` and
    `pulse_ms = 2.0`, energy above 4 kHz in the resulting pulse is
    much lower than below 1 kHz (rough sanity, not a tight bound).
  - [ ] **Active gating**: `is_active()` is false before any
    `trigger()`, true while the pulse runs, false again afterward.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] `just inner -p patches-drums` green.

## Notes

- `xorshift64` is already imported by [tom.rs](../../src/tom.rs#L49);
  reuse it for `FilteredClick`. The PRNG seed should derive from
  `instance_id` so two instances don't share noise sequences
  ([tom.rs:137](../../src/tom.rs#L137) is the existing pattern).
- The one-pole low-pass for `FilteredClick` is one multiply-add per
  sample: `y[n] = (1-α) · y[n-1] + α · x[n]` with
  `α = 1 - exp(-2π · lp_hz / sample_rate)`.
- Energy normalisation: the four shapes have different
  total-energy-per-pulse for the same `pulse_ms`. Document this in
  the module rustdoc and leave the call to per-shape gain calibration
  up to the consuming module (`Kick2` etc.) — they may want
  per-shape gain trims to keep the modules' loudness comparable
  across shape changes. Not in this ticket.
- This primitive is the design surface for the "excitation-shaped
  bridged-T" pattern described in the source conversation; getting
  the shapes right matters more than getting the menu exhaustive.
