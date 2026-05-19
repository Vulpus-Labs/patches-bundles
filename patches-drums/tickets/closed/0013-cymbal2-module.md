---
id: "0013"
title: Cymbal2 module — modal bank with body resonator and shimmer
priority: high
created: 2026-05-19
epic: E003
---

## Summary

Add `Cymbal2`, a cymbal voice built on `ModalBank` (ticket 0012)
plus a parallel low-frequency body resonator that adds the
gong-weight underneath the high partials. The crash content is the
modal bank struck by an excitation pulse; the body is one extra
`BridgedT` at ~160 Hz with a longer envelope, mixed in at low gain.
HP-filtered white noise mixes via `tone` matching the existing
[Cymbal](../../src/cymbal.rs) shape, and shimmer LFO on partial
frequencies routes through `ModalBank::tick_with_modulation`.

Sister to existing `Cymbal`; the two coexist.

## Acceptance criteria

- [ ] New module `patches-drums/src/cymbal2.rs`.
- [ ] Module name: `Cymbal2`. ABI version 1. Added to
  `patches_drums::register` and to the `export_modules!` invocation in
  [lib.rs](../../src/lib.rs#L41-L66).
- [ ] Ports (matching `Cymbal`'s convention):
  - [ ] `trigger` (mono, trigger).
  - [ ] `velocity` (mono, audio) — latched on trigger, defaults to
    1.0 when disconnected.
  - [ ] `out` (mono, audio).
- [ ] Realtime parameters:
  - [ ] `pitch` (float, 200–10000 Hz, default 600) — modal bank
    base frequency.
  - [ ] `decay` (float, 0.2–8.0 s, default 2.0) — outer envelope
    decay time multiplier. Note: per-partial Q already gives
    inhomogeneous decay; this scales the whole tail.
  - [ ] `decay_slope` (float, 0.0–1.0, default 0.5) — controls how
    much the per-partial Q profile slopes from low to high partial
    (0.0 = uniform, 1.0 = strongly decreasing → fast HF decay).
    Forwarded to `ModalBank::set_q_profile` by interpolating between
    a flat profile and the default decreasing one.
  - [ ] `tone` (float, 0.0–1.0, default 0.5) — modal bank vs HP-noise
    mix. Same role as [Cymbal](../../src/cymbal.rs#L24)'s `tone`.
  - [ ] `filter` (float, 2000–16000 Hz, default 6000) — noise
    highpass cutoff.
  - [ ] `shimmer` (float, 0.0–1.0, default 0.2) — slow LFO depth
    on partial frequencies (Hz scale internal — same convention as
    existing Cymbal's `mod_depth = shimmer * 20.0`).
  - [ ] `body_mix` (float, 0.0–1.0, default 0.3) — gain of the
    parallel low body resonator. 0 = no gong-weight; 1 = body equal
    to bank output.
  - [ ] `pulse_ms` (float, 0.1–10.0, default 1.0) — excitation
    duration. Short default — cymbals are struck sharply.
- [ ] Structural parameter:
  - [ ] `pulse_shape` (enum: `Dirac` | `ExpDecay` | `HalfSine` |
    `FilteredClick`, default `HalfSine`) — set at build time. Same
    pattern as `Kick2` at [kick2.rs:54](../../src/kick2.rs#L54).
- [ ] Process loop:
  - [ ] On trigger rising edge: latch velocity, reset modal bank
    + body resonator, fire excitation, reset shimmer LFO phase.
  - [ ] Each sample:
    1. `exc = excitation.tick()`.
    2. `bank_out = modal_bank.tick_with_modulation(exc, mod_depth, lfo_phase)`.
    3. `body_out = body_resonator.tick(exc, 0.0)`.
    4. `noise = highpassed white through SvfKernel` (same as existing
       Cymbal's [hp_filter](../../src/cymbal.rs#L64) path).
    5. `mix = bank_out * tone + noise * (1 - tone) + body_out * body_mix`.
    6. `output = mix * amp_env`. Scale by latched velocity →
       write to `out`.
    7. Advance shimmer LFO.
- [ ] Module rustdoc table mirrors the `Cymbal`-style description
  block at the top of [cymbal.rs:1–28](../../src/cymbal.rs#L1-L28).
  Document the body resonator's role explicitly — that the
  gong-weight comes from a parallel low BridgedT rather than from a
  partial of the modal bank.
- [ ] Unit tests in `cymbal2.rs`:
  - [ ] **Trigger produces output** — RMS > 0.001 over the first
    5000 samples.
  - [ ] **Long decay** — at `decay = 4.0`, RMS at samples 44000–45000
    is still > 0.001 (the cymbal rings past 1 s).
  - [ ] **HF dominant overall** — total HF (2–20 kHz) / total LF
    (20–500 Hz) > 4.0 across 4096 samples at default `body_mix`.
    Mirror [cymbal.rs:276–295](../../src/cymbal.rs#L276-L295).
  - [ ] **Tail darkens** — HF/total ratio over samples 4096–8192
    is measurably lower than over samples 0–2048 (per-partial Q
    profile is doing its work).
  - [ ] **Envelope peak is early** — peak windowed-RMS occurs in
    the first quarter of an 8192-sample run. Mirror
    [cymbal.rs:298–322](../../src/cymbal.rs#L298-L322).
  - [ ] **Shimmer changes output** — output at `shimmer = 1.0`
    differs from `shimmer = 0.0` by average sample diff > 0.001.
    Mirror [cymbal.rs:248–274](../../src/cymbal.rs#L248-L274).
  - [ ] **Body resonator audible** — total LF (60–250 Hz) energy
    at `body_mix = 1.0` is > 3× LF energy at `body_mix = 0.0`
    (body adds the gong-weight, and only the body resonator puts
    energy there).
  - [ ] **Velocity scales output** — half velocity ≈ half RMS,
    tolerance ±10%.
- [ ] FFI manifest test in [lib.rs:88](../../src/lib.rs#L88) updated
  to include `"Cymbal2"` in `EXPECTED_NAMES`.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.

## Notes

- The body resonator is a single `BridgedT` at ~160 Hz with `Q ≈ 5`
  (broad, not a tonal ringing thump). Use a longer outer envelope
  on its excitation path or just rely on the low Q producing a
  short body decay. Tune the defaults by ear before locking them
  in.
- `decay_slope`'s interpolation between flat and default Q profile:
  `q[i] = lerp(flat_q, default_q[i], slope)` where
  `flat_q = mean(default_q)` keeps the average decay roughly
  constant across slope settings — only the *shape* of the
  per-partial profile changes.
- The shimmer LFO advances per sample (one float add + wrap), same
  as existing [Cymbal](../../src/cymbal.rs#L193-L197). Keep the
  same ~3 Hz default.
- `pulse_shape = HalfSine` was chosen as default because a sharp
  but extended pulse strikes the bank cleanly without ringing the
  excitation itself; `Dirac` makes it too pingy.
- Stability: see ticket 0012's note on partial Q caps. Default
  parameter ranges keep us clear.
