---
id: "0021"
title: Voice quiescence gate — DecayEnvelope snap + per-voice process skip
priority: medium
created: 2026-05-19
---

## Summary

[`DecayEnvelope::tick`](../../src/primitives/envelope.rs#L40) multiplies
`level` by `decay_coeff` indefinitely — it asymptotes to zero but
never reaches it. Down-stream voices keep ticking their resonator
banks long after audible decay. Worst case is
[`Cymbal2`](../../src/cymbal2.rs) (8-second decay × 7 BridgedT
resonators per voice).

Two-part fix:

1. **Envelope-side.** Snap `level` to exactly `0.0` once it drops
   below an inaudible threshold (`1.0e-7`, ≈ -140 dBFS). Add
   [`DecayEnvelope::is_silent`](../../src/primitives/envelope.rs) →
   `bool`.

2. **Voice-side.** In each voice's `process`, if the envelope is
   silent *and* no trigger fired this sample, skip the bank /
   excitation / noise / mix work and write `0.0`. The PRNG state
   and LFO phase are intentionally *not* advanced — perceptually
   irrelevant for noise; the next trigger resets LFO phase anyway.

Bank state is implicitly safe: at -140 dBFS the bank's own ring is
also inaudible, and the next trigger calls `bank.reset_state()` /
`body.reset_state()` before excitation. Skipping ticks freezes the
state at its last value; the freeze is followed by a reset.

## Acceptance criteria

- [ ] [`DecayEnvelope::tick`](../../src/primitives/envelope.rs#L40)
      snaps `self.level` to `0.0` when `level < 1.0e-7` after the
      multiply step.
- [ ] [`DecayEnvelope`](../../src/primitives/envelope.rs#L6) exposes
      `pub fn is_silent(&self) -> bool { self.level == 0.0 }`.
- [ ] [`Cymbal2::process`](../../src/cymbal2.rs#L270) skips bank /
      body / excitation / noise / mix and writes `0.0` to the audio
      output when `!trigger_rose && self.amp_env.is_silent()`. The
      trigger-detect path *always* runs (so a rising edge always
      reaches the latch / reset / excitation block).
- [ ] [`ClosedHiHat2::process`](../../src/hihat2.rs#L232) and
      [`OpenHiHat2::process`](../../src/hihat2.rs#L428) gain the same
      skip. (The choke path on `OpenHiHat2` must still run when the
      envelope is silent — choke before is-silent check.)
- [ ] [`XorCymbal::process`](../../src/xor_cymbal.rs#L176),
      [`XorClosedHiHat::process`](../../src/xor_hihat.rs#L164), and
      [`XorOpenHiHat::process`](../../src/xor_hihat.rs#L330) gain the
      same skip.
- [ ] Audit existing voices that use `DecayEnvelope`:
      [`Kick`](../../src/kick.rs), [`Kick2`](../../src/kick2.rs),
      [`Snare`](../../src/snare.rs), [`Clap`](../../src/clap.rs),
      [`Tom`](../../src/tom.rs), [`Tom2`](../../src/tom2.rs),
      [`Cymbal`](../../src/cymbal.rs),
      [`ClosedHiHat` / `OpenHiHat`](../../src/hihat.rs),
      [`Claves`](../../src/claves.rs),
      [`Claves2`](../../src/claves2.rs). Verify the envelope snap
      does not break any of their tests (it cannot — they all
      depend on `level → 0` as monotone non-increasing, which the
      snap preserves). Optional: extend the skip pattern to those
      voices in this ticket or a follow-up.
- [ ] New unit test on `DecayEnvelope`: after enough ticks for the
      analytical level to drop below `1.0e-7`, `is_silent()` returns
      `true` and `level` equals exactly `0.0`.
- [ ] New regression test on `Cymbal2` (or one voice): trigger,
      tick past the snap point, assert RMS of next 1024 samples is
      exactly `0.0` (not just small).
- [ ] All existing tests pass — the
      [decay_envelope_monotonically_decreasing](../../src/primitives/envelope.rs#L114)
      test continues to pass (snap is monotone).
- [ ] `cargo test -p patches-drums` green.

## Notes

- The existing
  [`decay_envelope_decays_over_time`](../../src/primitives/envelope.rs#L78)
  test asserts `v < 0.01` after one decay-time — well above the
  `1.0e-7` snap threshold, so it's unaffected.
- The shimmer LFO not advancing while a voice is silent is fine —
  next trigger resets `lfo_phase = 0.0` anyway
  ([cymbal2.rs:282](../../src/cymbal2.rs#L282)).
- Choke-while-silent: `OpenHiHat2::choke` just sets `level = 0.0`,
  and the env is already `0.0` when silent — no-op. The skip
  ordering (`choke_rose` handled before `is_silent` check) is
  belt-and-braces for correctness if the choke logic ever grows
  side effects.
