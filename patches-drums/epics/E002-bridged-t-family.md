---
id: E002
title: Bridged-T resonator family — Kick2, Tom2, Claves2
status: open
created: 2026-05-18
adr: 0002
---

## Goal

Add three new modules to `patches-drums` — `Kick2`, `Tom2`, `Claves2`
— built around a struck high-Q resonator with soft-clipped feedback,
exciting the resonator with a short shaped pulse rather than driving
it with a phase-accumulator oscillator. These coexist with the
existing `Kick`, `Tom`, `Claves` rather than replacing them. See
[ADR 0002](../adrs/0002-bridged-t-resonator-family.md).

The bundle today is built around the driven-oscillator school for
its pitched voices. This epic adds the struck-resonator school as a
second paradigm: smaller parameter surface, decay-coupled-to-Q,
emergent pitch droop from feedback nonlinearity, character carried by
the excitation shape and the clipper rather than by independent
envelopes.

## Scope

- New primitive `BridgedT` in
  `patches-drums/src/primitives/bridged_t.rs`. Wraps
  `patches_dsp::SvfKernel` with a soft-clipped feedback path. Bandpass
  tap output.
- New primitive `Excitation` in
  `patches-drums/src/primitives/excitation.rs`. Short shaped pulse
  generator. Shapes: `Dirac`, `ExpDecay`, `HalfSine`,
  `FilteredClick`.
- New module `Kick2` in `patches-drums/src/kick2.rs`. Excitation →
  BridgedT(tune ≈ 55 Hz, Q ≈ 50) → bandpass tap. Sister to existing
  `Kick`.
- New module `Tom2` in `patches-drums/src/tom2.rs`. Excitation →
  BridgedT(tune ≈ 150 Hz, Q ≈ 30) → bandpass tap. Sister to existing
  `Tom`.
- New module `Claves2` in `patches-drums/src/claves2.rs`. Two
  `BridgedT` stages: stage 1 excited by the trigger, stage 1's bp
  output (via rising-edge detector) re-excites stage 2. `cascade_mix`
  blends stage-1-only with stage-1+stage-2.
- All three modules registered by `patches_drums::register` and
  exported through `export_modules!`. ABI version 1.
- Per-module unit tests in the same crate, mirroring the conventions
  in [kick.rs](../src/kick.rs#L252) and [tom.rs](../src/tom.rs#L199).

## Out of scope

- A bridged-T conga or cowbell. Plausible follow-up; not this epic.
- Modal / banked-resonator pitched voices. Separate ADR.
- FM/PD kick. Cross-bundle dependency on Prism.
- Replacing or deprecating the existing `Kick`, `Tom`, `Claves`. The
  two families coexist.
- External-signal excitation (sending audio in instead of a
  synthesised pulse). Possible later extension.
- Per-voice CV over `clip` or `q`. Realtime params suffice for v1.

## Tickets

- [0006 — `BridgedT` primitive: SVF wrapper with soft-clipped feedback](../tickets/open/0006-bridged-t-primitive.md)
- [0007 — `Excitation` primitive: short shaped pulse generator](../tickets/open/0007-excitation-primitive.md)
- [0008 — `Kick2` module: struck resonator at sub-bass tune](../tickets/open/0008-kick2-module.md)
- [0009 — `Tom2` module: struck resonator at tom tune with amplitude-driven droop](../tickets/open/0009-tom2-module.md)
- [0010 — `Claves2` module: cascaded struck resonator with re-excitation](../tickets/open/0010-claves2-module.md)

## Acceptance

- `Kick2`, `Tom2`, `Claves2` are registered by
  `patches_drums::register` and appear in the FFI manifest with ABI
  version 1.
- A `.patches` file using each module produces audible, decaying
  percussion on trigger.
- `Kick2`'s pitch droop with `clip > 0` is measurable: the dominant
  bin of the first 5 ms is higher than the dominant bin of the next
  20 ms (steady-state ω₀).
- `Claves2` with `cascade_mix = 0` is bit-identical (within float
  tolerance) to a single-stage equivalent; with `cascade_mix = 1` the
  output has measurably longer ringing tail than `cascade_mix = 0` at
  the same `q`.
- No allocations on the audio thread under release-mode profiling.
- All four tiers (`inner` / `commit` / `push` / `smoke`) green.
