---
id: "0011"
title: Extract StruckResonatorVoice helper shared by Kick2 and Tom2
priority: medium
created: 2026-05-18
epic: E002
---

## Summary

`Kick2` and `Tom2` (tickets 0008 and 0009) ended up with near-identical
process-loop spines: same `BridgedT` + `Excitation` + `AttackFmPulse`
ownership, same `diode()` shaper, same Plaits-derived FM constants, same
trigger / per-sample tick sequence. They differ only in parameter
ranges, defaults, the `voct` port (Kick2 only), and the rustdoc framing.

Ticket 0009 anticipated this and set the bar: *"wait until two call
sites exist and the actual shared shape is visible"*. Both call sites
now exist and the shape is stable. Extract the shared spine into a
crate-local helper so the modules can shrink to thin descriptor +
parameter shells.

`Claves2` deliberately does not share this spine (two cascaded stages
with an edge-detector trigger relationship) and stays unchanged.

## Acceptance criteria

- [ ] New module `patches-drums/src/primitives/struck_resonator_voice.rs`
  exposes:
  - [ ] `pub struct StruckResonatorVoice` holding the `BridgedT`,
    `Excitation`, `AttackFmPulse`, current `drive`, current `attack`,
    and the latched velocity.
  - [ ] `StruckResonatorVoice::new(sample_rate: f32, tune_hz: f32,
    q: f32, pulse_shape: PulseShape, pulse_ms: f32, instance_seed:
    u64) -> Self`.
  - [ ] `set_tune`, `set_q`, `set_pulse_ms`, `set_drive`, `set_attack`
    setters that mirror the per-parameter handling currently in
    `Kick2`/`Tom2` (notably: `set_drive` also passes the value through
    to `BridgedT::set_clip`, keeping the saturator coupled to the
    drive knob).
  - [ ] `trigger(velocity: f32)` — resets resonator state, fires the
    excitation, fires the attack-FM pulse, latches velocity.
  - [ ] `tick() -> f32` — runs one sample of the full spine and
    returns the velocity-scaled bp output. Cost target: one
    excitation tick + one resonator tick + one attack-FM tick + a
    small constant of arithmetic, same as the current inline code.
  - [ ] Private `AttackFmPulse` struct moved here (currently
    duplicated in `kick2.rs` and `tom2.rs`).
  - [ ] Private `diode` helper moved here.
- [ ] `pub use struck_resonator_voice::StruckResonatorVoice` added to
  [primitives/mod.rs](../../src/primitives/mod.rs); `AttackFmPulse`
  and `diode` stay private.
- [ ] [`Kick2`](../../src/kick2.rs) refactored to own a
  `StruckResonatorVoice` instead of the four separate fields. The
  module retains its `voct` handling (overrides `set_tune` via
  `periodic_update`) and its parameter descriptor; everything else
  delegates to the voice.
- [ ] [`Tom2`](../../src/tom2.rs) refactored the same way.
- [ ] [`Claves2`](../../src/claves2.rs) is **not** refactored — its
  two-stage cascade does not share the spine and the asymmetric
  shape is the design point.
- [ ] All existing unit tests in `kick2.rs` and `tom2.rs` continue to
  pass without modification. Behaviour (sample output, parameter
  effects, droop direction, voct override) must be unchanged.
- [ ] No new tests in `struck_resonator_voice.rs` — the voice is
  exercised by the module tests.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] `cargo test -p patches-drums` and `cargo clippy --workspace
  --all-targets -- -D warnings` both green.

## Notes

- The voice owns the `BridgedT`, `Excitation`, and `AttackFmPulse`;
  the modules own the `Module` trait impl, the parameter descriptor,
  and the port wiring. This is the right cut — anything that depends
  on `patches_sdk` types stays in the module, anything that is pure
  DSP-state-machine moves to the voice.
- The Plaits FM constants (`SELF_FM_REF = 0.08`, `ATTACK_FM_REF = 1.7`,
  `FM_PULSE_SECS = 6.0e-3`, `FM_PULSE_FILTER_SECS = 0.1e-3`) move into
  `struck_resonator_voice.rs` alongside the spine.
- Do not introduce a builder pattern or a config struct for
  construction — the constructor has six arguments but they are all
  required and the call sites (Kick2 and Tom2 `prepare`) are easy to
  read. Adding a builder is over-engineering for two call sites.
- Sample-rate caching: the voice needs `sample_rate` only for
  `AttackFmPulse::new`; after construction the resonator and
  excitation hold their own copies. Don't store `sample_rate` on the
  voice struct if nothing uses it after `new`.
