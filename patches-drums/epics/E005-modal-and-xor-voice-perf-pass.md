---
id: E005
title: Modal and XOR voice perf pass — fast tan, state preservation, voice gating
status: closed
created: 2026-05-19
---

## Goal

Tidy and tune the new `Cymbal2` / `ClosedHiHat2` / `OpenHiHat2`
modal-bank voices ([E003](E003-modal-metal-voices.md)) and the
`XorCymbal` / `XorClosedHiHat` / `XorOpenHiHat` XOR-pair voices
([E004](E004-xor-metal-voices.md)) for CPU and correctness in one
focused review pass:

- per-sample `tan` in `TptSvf` replaced with `fast_tan_small` (new
  in `patches-dsp` 0.6.1),
- HP-noise filter coefficient updates preserve filter state,
- xor-pair-tone hot loop drops a dead branch and short-circuits
  when modulation depth is zero,
- voices early-out from `process` once the amp envelope has
  decayed past audibility,
- realtime-param updates gate downstream side effects on
  value-changed.

Net effect: meaningfully cheaper Cymbal2 (the worst case — 7
resonators + long decay), cleaner code on the hot path, and a
state-correctness fix for the noise filter restart click. No
audible change to the established voice character.

## Scope

- Bump workspace `patches-dsp` dep to 0.6.1 (adds
  `patches_dsp::approximate::fast_tan_small`).
- `TptSvf`: cache `pi_over_sr`, swap libm `tan` for `fast_tan_small`.
- `Cymbal2` / `ClosedHiHat2` / `OpenHiHat2` / `XorCymbal` /
  `XorClosedHiHat` / `XorOpenHiHat`: update their HP-noise filter
  via `SvfKernel::set_static` rather than replacing the kernel.
- `XorPairTone::tick_with_modulation`: drop unreachable `phase < 0`
  rebase; early-out to `tick` when `mod_depth == 0`.
- `DecayEnvelope`: snap `level` to exactly `0.0` once below
  `1.0e-7`; expose `is_silent()`. All six modal/XOR voices skip
  bank/excitation/noise/mix work when silent (after handling any
  rising trigger or choke).
- All six modules: cache last-applied param values and skip
  downstream side effects (bank rewrites, filter coeff recompute,
  excitation pulse_ms write) when the value hasn't changed.

## Out of scope

- Adopting `CoefRamp` block-rate ramped coefficients for `TptSvf` /
  `ModalBank`. Considered and recorded as
  [0023 — wontfix](../tickets/closed/0023-modal-bank-coef-ramp.md):
  after `fast_tan_small` lands, the per-sample-tan cost was the
  only meaningful pre-amortisation, and ramping wouldn't help the
  self-FM (`Kick2` / `Tom2`) callers that need per-sample anyway.
  Re-open trigger documented on the ticket.
- Extending the envelope-snap quiescence gate to the other
  `DecayEnvelope` consumers (`Kick`, `Snare`, `Clap`, `Tom`,
  `Claves`, etc.). The snap is in place; voice-side skip is
  follow-up.
- Replacing the HP-noise `SvfKernel` static path with a CV-able
  ramped version. Possible follow-up if `filter` ever becomes a
  CV-able input.
- Performance benchmarks. The end-to-end win is qualitatively
  measurable on the `drum_machine2` example; no benchmark gate is
  added here.

## Tickets

- [0018 — `TptSvf` uses `fast_tan_small` and caches `pi_over_sr`](../tickets/closed/0018-fast-tan-tpt-svf.md)
- [0019 — Cymbal2 / HiHat2 / Xor\* preserve HP-noise filter state](../tickets/closed/0019-hp-filter-state-preservation.md)
- [0020 — `XorPairTone` dead-branch removal + zero-depth shortcut](../tickets/closed/0020-xor-pair-tone-cleanup.md)
- [0021 — Voice quiescence gate (envelope snap + per-voice process skip)](../tickets/closed/0021-voice-quiescence-gate.md)
- [0022 — Cymbal2 / HiHat2 / Xor\* param-frame rebuild guards](../tickets/closed/0022-param-frame-rebuild-guards.md)
- [0023 — `ModalBank` / `TptSvf` `CoefRamp` adoption (wontfix)](../tickets/closed/0023-modal-bank-coef-ramp.md)

## Acceptance

- `patches-dsp` workspace dep pinned at `0.6.1`.
- `TptSvf::set_f_q` no longer calls libm `tan`.
- All six modal/XOR voices use `set_static` (not `new_static`
  replacement) for the HP-noise filter and gate downstream work in
  `update_validated_parameters` on value-changed.
- `DecayEnvelope::tick` snaps to exactly `0.0` below the silence
  threshold and exposes `is_silent`.
- `cargo test -p patches-drums` green, including new regression
  tests for envelope snap, zero-depth XorPairTone equivalence,
  Cymbal2 silent-after-snap, and Cymbal2 idempotent param updates.
- `cargo clippy -p patches-drums -- -D warnings` clean.
