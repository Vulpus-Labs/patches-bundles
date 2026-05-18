---
id: "0009"
title: Tom2 module — struck resonator at tom tune with amplitude-driven droop
priority: medium
created: 2026-05-18
epic: E002
---

## Summary

Add `Tom2`, a struck-resonator tom built on the same `BridgedT` and
`Excitation` primitives as `Kick2`. Differences from `Kick2`: defaults
(higher tune, lower Q, shorter pulse, stronger default `drive`) and
rustdoc framing — the self-FM-driven pitch glide is the tom's
signature, analogous to the amplitude-driven pitch envelope of the
early analog tom circuits where the bridged-T's pole locations
shifted under the op-amp's nonlinear loop response.

The pitch-droop mechanism is identical to Kick2's: explicit
amplitude → frequency FM via the resonator's lp tap, plus an
attack-FM pulse on trigger. See
[ADR 0002 §"Nonlinearity and pitch droop"](../../adrs/0002-bridged-t-resonator-family.md#nonlinearity-and-pitch-droop).

Sister to existing [Tom](../../src/tom.rs); the two coexist.

## Acceptance criteria

- [ ] New module `patches-drums/src/tom2.rs`.
- [ ] Module name: `Tom2`. ABI version 1. Added to
  `patches_drums::register` and to the `export_modules!` invocation in
  [lib.rs](../../src/lib.rs#L41-L66).
- [ ] Ports (mirroring `Tom`'s convention — `Tom` does not have a
  `voct` input today, so `Tom2` follows suit):
  - [ ] `trigger` (mono, trigger).
  - [ ] `velocity` (mono, audio) — latched on trigger, defaults to
    1.0 when disconnected.
  - [ ] `out` (mono, audio).
- [ ] Realtime parameters:
  - [ ] `tune` (float, 40–500 Hz, default 150) — resonator centre
    frequency.
  - [ ] `q` (float, 5.0–60.0, default 30) — resonator Q.
  - [ ] `pulse_ms` (float, 0.1–8.0, default 1.5) — excitation
    duration.
  - [ ] `drive` (float, 0.0–1.0, default 0.5) — self-FM depth plus
    output-saturator amount (fixed internal ratio). Larger default
    than `Kick2` so the amplitude-coupled glide is audibly present
    at defaults — the tom's signature.
  - [ ] `attack` (float, 0.0–1.0, default 0.7) — attack-FM depth.
    Tom strikes have a sharper transient than kicks, so the default
    is correspondingly bigger.
- [ ] Structural parameter:
  - [ ] `pulse_shape` (enum: `Dirac` | `ExpDecay` | `HalfSine` |
    `FilteredClick`, default `ExpDecay`) — set at build time. A
    sharper default than `Kick2`'s `HalfSine` because the tom's
    transient is shorter and more click-like.
- [ ] Process loop: identical structure to `Kick2` ticket 0008,
  including the per-sample attack-FM pulse + self-FM (Diode shaper
  on `bridged_t.lp()`) → `fm_offset` → `bridged_t.tick(pulse,
  fm_offset)` chain. The only differences from Kick2 are parameter
  defaults and the lack of a `voct` port.
- [ ] Module rustdoc table mirrors the `Tom`-style description block
  at the top of [tom.rs:1–28](../../src/tom.rs#L1-L28). State clearly
  that the pitch droop is amplitude-driven and emerges from `clip`,
  not from a separate pitch envelope.
- [ ] Unit tests in `tom2.rs`:
  - [ ] **Trigger produces output** — RMS > 0.01 over the first 2000
    samples.
  - [ ] **Pitch tracking** — dominant FFT bin at `tune = 300 Hz` is
    higher than at `tune = 80 Hz`. Mirror
    [tom.rs:270–298](../../src/tom.rs#L270-L298).
  - [ ] **Decay** — RMS at samples 20000–22000 < 0.005 at default
    params.
  - [ ] **Envelope monotonicity** — windowed RMS over 8192 samples
    is non-increasing within 5% tolerance. Mirror
    [tom.rs:300–325](../../src/tom.rs#L300-L325).
  - [ ] **Droop with attack-FM** — first 200 samples after trigger
    have a higher dominant bin than samples 2000–6000 when `attack =
    0.8`; with `attack = 0.0` and `drive = 0.0` the two windows
    match within ±2 bins.
  - [ ] **Velocity scales output** — half velocity ≈ half RMS,
    tolerance ±10%.
- [ ] FFI manifest test in [lib.rs:88](../../src/lib.rs#L88) updated
  to include `"Tom2"` in `EXPECTED_NAMES`.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.

## Notes

- Refactor opportunity: `Kick2` and `Tom2` share essentially all
  process-loop logic and differ only in parameter ranges and
  defaults. If after both modules land the duplication feels real,
  open a follow-up to extract a shared `StruckResonatorVoice`
  helper. Do not pre-extract — wait until two call sites exist and
  the actual shared shape is visible (a third module landing
  `Claves2` may change what wants to be shared).
- The existing `Tom`'s noise-attack layer is intentionally absent
  from `Tom2`. The transient character in this family comes from the
  excitation shape (which can include a `FilteredClick` if the user
  wants a noisy attack). Don't smuggle a parallel noise generator
  back in.
- Stability: TPT SVF is unconditionally stable; `BridgedT::tick`
  clamps `fm_offset` so `f` stays under `0.4 · sr`. No clip-induced
  limit cycles to worry about, unlike the earlier draft.
