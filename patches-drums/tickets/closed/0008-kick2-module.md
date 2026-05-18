---
id: "0008"
title: Kick2 module — struck resonator at sub-bass tune
priority: high
created: 2026-05-18
epic: E002
---

## Summary

Add `Kick2`, a struck-resonator kick built on the `BridgedT` and
`Excitation` primitives from tickets 0006 and 0007. Architecture is:
trigger fires an excitation pulse → pulse strikes a high-Q bridged-T
tuned around 55 Hz → bandpass tap is the output. No separate
amplitude envelope, no separate click layer — decay is the
resonator's Q, attack character is the excitation shape. Pitch
behaviour comes from two explicit FM paths driven by the voice (see
[ADR 0002 §"Nonlinearity and pitch droop"](../../adrs/0002-bridged-t-resonator-family.md#nonlinearity-and-pitch-droop)):

- **Self-FM** (`drive` param): half-wave-rectified lp tap of the
  resonator scales the current cutoff. Amplitude-coupled — pitch
  lifts at high amp, settles back to `ω₀` as the ring decays. Also
  scales the output saturator amount.
- **Attack-FM** (`attack` param): short rectangular pulse (≈ 6 ms,
  one-pole-smoothed) at trigger adds an extra brief lift to the
  cutoff. Trigger-locked, not amplitude-locked.

Sister to existing [Kick](../../src/kick.rs); the two coexist.

## Acceptance criteria

- [ ] New module `patches-drums/src/kick2.rs`.
- [ ] Module name: `Kick2`. ABI version 1. Added to
  `patches_drums::register` and to the `export_modules!` invocation in
  [lib.rs](../../src/lib.rs#L41-L66).
- [ ] Ports (matching `Kick`'s convention so the modules are
  interchangeable):
  - [ ] `trigger` (mono, trigger) — rising edge excites the
    resonator.
  - [ ] `voct` (mono, audio) — 1V/oct pitch CV; overrides `tune` when
    connected, using the `C0_FREQ` reference from
    [kick.rs:37](../../src/kick.rs#L37).
  - [ ] `velocity` (mono, audio) — latched on trigger, scales output;
    defaults to 1.0 when disconnected. Mirror the latching pattern
    from [kick.rs:199–204](../../src/kick.rs#L199-L204).
  - [ ] `out` (mono, audio) — kick signal.
- [ ] Realtime parameters:
  - [ ] `tune` (float, 20–200 Hz, default 55) — resonator centre
    frequency.
  - [ ] `q` (float, 5.0–80.0, default 50) — resonator Q. Decay length
    follows from Q.
  - [ ] `pulse_ms` (float, 0.1–10.0, default 2.0) — excitation
    duration.
  - [ ] `drive` (float, 0.0–1.0, default 0.3) — self-FM depth plus
    output-saturator amount (fixed internal ratio). Amplitude-coupled
    pitch droop and harmonic dirt.
  - [ ] `attack` (float, 0.0–1.0, default 0.5) — attack-FM depth.
    Strike-locked pitch lift over ≈ 6 ms.
- [ ] Structural parameter:
  - [ ] `pulse_shape` (enum: `Dirac` | `ExpDecay` | `HalfSine` |
    `FilteredClick`, default `HalfSine`) — selected at build time.
    Use [params.rs](../../../patches-fft-bundle/src/convolution_reverb/params.rs)
    in the FFT bundle as the template for enum structural params.
- [ ] Process loop:
  - [ ] On trigger rising edge: latch velocity, reset the resonator
    state, call `excitation.trigger()`, reset the attack-FM pulse
    counter (≈ 6 ms = `0.006 · sr` samples) and its one-pole smoother
    state.
  - [ ] Each sample:
    1. Compute attack-FM: while pulse counter > 0, raw FM = 1.0,
       else 0.0; smooth through a one-pole LP with ≈ 0.1 ms time
       constant (matches Plaits's `kPulseFilterTime`). Result is
       `attack_fm_lp`.
    2. Compute self-FM from `bridged_t.lp()` (the previous tick's
       lp tap) via Plaits's `Diode` half-wave shaper:
       `punch = 0.7 + diode(10.0 * lp - 1.0)`. Result is `self_fm`.
    3. `fm_offset = drive * 0.08 * self_fm + attack * 1.7 *
       attack_fm_lp`. The `0.08` and `1.7` are Plaits's reference
       depths and keep `fm_offset` in the sane modulation range.
    4. `pulse = excitation.tick()`.
    5. `ring = bridged_t.tick(pulse, fm_offset)`.
    6. Output: `ring * velocity`.
  - [ ] No envelopes — decay is the SVF ring; no click layer — the
    transient is the excitation pulse; no separate sine VCO — the
    resonator is the body.
- [ ] V/oct handling: mirror
  [kick.rs:239–247](../../src/kick.rs#L239-L247) — `periodic_update`
  reads `voct` when connected and calls `bridged_t.set_tune` with
  `C0_FREQ * fast_exp2(voct)`.
- [ ] Module rustdoc table mirrors the `Kick`-style description block
  at the top of [kick.rs:1–32](../../src/kick.rs#L1-L32).
- [ ] Unit tests in `kick2.rs`:
  - [ ] **Trigger produces audible output** — RMS over first 2000
    samples after trigger > 0.01.
  - [ ] **Decay to silence** — with default params, RMS at samples
    20000–22000 < 0.005 (resonator has rung down).
  - [ ] **Tune affects pitch** — dominant FFT bin at `tune = 80 Hz`
    is higher than at `tune = 40 Hz`. Use the existing
    [test_support](../../src/test_support.rs) `dominant_bin`/
    `magnitude_spectrum` helpers, same pattern as
    [tom.rs:270–298](../../src/tom.rs#L270-L298).
  - [ ] **Q affects decay length** — RMS at sample 8000 is higher
    for `q = 70` than for `q = 20`.
  - [ ] **Pitch droop with attack-FM** — first 200 samples after
    trigger have a **higher** dominant bin than samples 2000–6000
    when `attack = 0.8` (pitch lifts at strike, settles to `ω₀`);
    with `attack = 0.0` and `drive = 0.0` the two windows match
    within ±2 bins.
  - [ ] **Pitch droop with self-FM (drive)** — same test shape with
    `drive = 0.8` and a high-Q setting (q ≥ 60) so the resonator
    bp/lp amplitude is large enough to engage the self-FM path;
    early-window dominant bin is higher than late-window.
  - [ ] **Velocity scales output** — half velocity ≈ half RMS,
    tolerance ±10%. Mirror
    [kick.rs:334–362](../../src/kick.rs#L334-L362).
  - [ ] **Velocity defaults to 1.0 when disconnected** — output
    audible without a velocity connection.
  - [ ] **V/oct overrides tune** — pattern as
    [kick.rs:374–430](../../src/kick.rs#L374-L430).
- [ ] FFI manifest test in [lib.rs:88](../../src/lib.rs#L88) updated
  to include `"Kick2"` in `EXPECTED_NAMES`.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.

## Notes

- The parameter surface is intentionally smaller than `Kick` (5 vs 6
  realtime params). Do not smuggle a separate pitch envelope or click
  layer back in for "parity" — the FM paths replace what an explicit
  pitch envelope would do, and the excitation pulse replaces the
  click layer. The asymmetry is the design point per
  [ADR 0002 §"Consequences/Negative"](../../adrs/0002-bridged-t-resonator-family.md#negative).
- Pitch droop direction is **upward at attack**, settling to `ω₀` —
  matches the 808 ("thump → boom"). Both `drive` and `attack` raise
  `f` from `f0`; neither drops below `f0`.
- The `0.08` self-FM coefficient and `1.7` attack-FM coefficient are
  taken directly from Plaits's `AnalogBassDrum` and represent
  Émilie's calibrated depths. We expose `drive` and `attack` as
  multipliers in `[0, 1]` on those.
- Stability: per-sample `set_f_q` on a TPT SVF is unconditionally
  stable; the only practical concern is `fm_offset` getting large
  enough that `f` approaches Nyquist. `BridgedT::tick` clamps
  internally so the module does not need to worry.
- Keep the `set_ports` shape consistent with `Kick` so a swap-in is
  a single name change in a `.patches` file.
