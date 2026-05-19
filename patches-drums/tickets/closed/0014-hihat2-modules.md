---
id: "0014"
title: ClosedHiHat2 + OpenHiHat2 modules — modal bank with choke
priority: medium
created: 2026-05-19
epic: E003
---

## Summary

Add `ClosedHiHat2` and `OpenHiHat2` — hi-hat voices built on
`ModalBank` (ticket 0012) with the same architecture as `Cymbal2`
but at hi-hat tuning, shorter default decays, and (for the open
variant) a `choke` input. No body resonator — hi-hats don't have
the cymbal's gong-weight.

Both modules live in one file (`hihat2.rs`) mirroring the existing
[hihat.rs](../../src/hihat.rs) which houses
`ClosedHiHat` and `OpenHiHat` together. Sister to those modules; all
four coexist.

## Acceptance criteria

- [ ] New module `patches-drums/src/hihat2.rs` exporting both
  `ClosedHiHat2` and `OpenHiHat2`.
- [ ] Module names: `ClosedHiHat2`, `OpenHiHat2`. ABI version 1.
  Added to `patches_drums::register` and to the `export_modules!`
  invocation in [lib.rs](../../src/lib.rs#L41-L66).
- [ ] Shared shape (both modules):
  - [ ] Ports `trigger`, `velocity`, `out` exactly as
    [ClosedHiHat](../../src/hihat.rs#L48). `OpenHiHat2` adds
    `choke` (mono, trigger) matching the existing
    [OpenHiHat:223](../../src/hihat.rs#L223).
  - [ ] Realtime parameters `pitch`, `decay`, `decay_slope`, `tone`,
    `filter`, `pulse_ms`. Defaults:
    - `ClosedHiHat2`: `pitch = 400`, `decay = 0.04`, `decay_slope = 0.7`,
      `tone = 0.5`, `filter = 8000`, `pulse_ms = 0.5`.
    - `OpenHiHat2`: `pitch = 400`, `decay = 0.4`, `decay_slope = 0.5`,
      `tone = 0.5`, `filter = 7000`, `pulse_ms = 0.5`.
    `decay_slope` semantics identical to `Cymbal2`'s: 0 = flat
    Q profile, 1 = strongly decreasing per-partial Q.
  - [ ] Structural parameter `pulse_shape` (enum) — defaults to
    `HalfSine` for both. Same pattern as `Kick2` at
    [kick2.rs:54](../../src/kick2.rs#L54).
- [ ] Process loop per sample (both modules):
  1. `exc = excitation.tick()`.
  2. `bank_out = modal_bank.tick(exc)` (no shimmer modulation on
     hi-hats — that's a cymbal-only thing).
  3. `noise = highpassed white through SvfKernel` matching the
     existing [hihat.rs:hp_filter](../../src/hihat.rs#L59).
  4. `mix = bank_out * tone + noise * (1 - tone)`.
  5. `output = mix * amp_env`. Scale by latched velocity → write
     to `out`.
- [ ] On trigger rising edge: latch velocity, reset modal bank,
  fire excitation.
- [ ] `OpenHiHat2` `choke` handling: on `choke` rising edge, call
  `amp_env.choke()` (existing helper used by
  [OpenHiHat:334](../../src/hihat.rs#L334)). Modal bank state is
  not reset — let the envelope cut the output, which is the same
  contract as the existing `OpenHiHat`.
- [ ] Module rustdoc tables mirror the existing hi-hat description
  blocks at [hihat.rs:1–26](../../src/hihat.rs#L1-L26) and
  [hihat.rs:182–198](../../src/hihat.rs#L182-L198). Note that the
  modal-bank variant decays inhomogeneously (high partials fade
  first), which is audibly different from the existing
  metallic-tone hi-hats.
- [ ] Unit tests in `hihat2.rs`:
  - [ ] **ClosedHiHat2 trigger produces output** — RMS > 0.01
    over the first 1000 samples.
  - [ ] **ClosedHiHat2 short decay** — RMS at samples 8000–9000 <
    0.005 at default params.
  - [ ] **OpenHiHat2 long decay** — at `decay = 0.4`, RMS at
    samples 8000–9000 is > 0.005 (still ringing); at 44000–45000 <
    0.005 (decayed).
  - [ ] **OpenHiHat2 choke silences** — trigger, run 2000 samples,
    measure RMS > 0.001; fire `choke`, run 4000 samples, measure
    RMS < 0.001. Mirror
    [hihat.rs:418–448](../../src/hihat.rs#L418-L448).
  - [ ] **HF dominant** — total HF (4–16 kHz) / total LF
    (20–500 Hz) > 5.0 across 2048 samples after trigger.
  - [ ] **Tail darkens (OpenHiHat2)** — HF/total ratio over
    samples 4096–8192 lower than over samples 0–2048. Closed hat
    is too short for this test; open hat carries it.
  - [ ] **Velocity scales output** — half velocity ≈ half RMS,
    tolerance ±10%, on both modules.
- [ ] FFI manifest test in [lib.rs:88](../../src/lib.rs#L88) updated
  to include `"ClosedHiHat2"` and `"OpenHiHat2"` in
  `EXPECTED_NAMES`.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.

## Notes

- Two modules in one file is the established pattern from
  [hihat.rs](../../src/hihat.rs). Keep it. The shared `module_params!`
  macro will need two invocations (one per module) since their
  parameters are identical but their module identities are
  distinct.
- No shimmer LFO on hi-hat voices. Cymbal2 has it; these don't,
  matching the existing `Cymbal` vs `ClosedHiHat` / `OpenHiHat`
  asymmetry.
- The `decay_slope` defaults differ: closed hat skews higher
  (0.7) for a tighter HF-fading tick, open hat is more balanced
  (0.5) so the long ring carries more body. Tune by ear when the
  module lands.
- Choke ramp: existing `DecayEnvelope::choke()` produces a fast
  fade rather than an instant cut. That's the contract — keep
  it.
- The closed/open distinction is purely envelope timing — same
  modal bank, same pulse shape, same partial profile. The "real
  hi-hat sounds different open vs closed" effect comes mostly
  from decay time, and the rest is from the noise mix and HP
  cutoff which are user-controlled.
