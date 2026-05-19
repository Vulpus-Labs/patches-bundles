---
id: "0022"
title: Cymbal2 / HiHat2 / Xor* — guard param-frame work on value change
priority: low
created: 2026-05-19
---

## Summary

[`Cymbal2::update_validated_parameters`](../../src/cymbal2.rs#L243)
unconditionally:

- rebuilds the HP-noise filter (one `tan`, one `sqrt`, one ladder
  call inside `q_to_damp`),
- recomputes the per-partial Q profile (`q_profile_for_slope`) and
  pushes it into all six bank resonators,
- pushes `pitch` into the bank (`set_base_freq` → six
  `BridgedT::set_tune` calls),
- pushes excitation `pulse_ms`.

This runs every param frame regardless of whether any of those
values changed. The host pushes a param frame every periodic-update
interval (default 16–32 samples), so the rebuild fires hundreds of
times per second per voice even when the user is not touching
controls.

Same pattern in
[`ClosedHiHat2`](../../src/hihat2.rs#L208),
[`OpenHiHat2`](../../src/hihat2.rs#L403),
[`XorCymbal`](../../src/xor_cymbal.rs#L148),
[`XorClosedHiHat`](../../src/xor_hihat.rs#L139),
[`XorOpenHiHat`](../../src/xor_hihat.rs#L304).

Cache the last-applied value for each param and skip the downstream
work when unchanged. Block-rate not sample-rate, so the win is
modest — but the diff is small and removes a constant tax.

This ticket lands *on top of* ticket 0019 (state-preserving
`set_static`) — the per-frame guard then *also* avoids unnecessary
coefficient writes.

## Acceptance criteria

- [ ] All six modules listed above gate the following work on a
      cached-value-changed check (last-applied vs new):
  - [ ] `filter` → HP-filter coefficient update
        (already addressed by `set_static` in ticket 0019; this
        ticket adds the change guard around it).
  - [ ] `pitch` → `bank.set_base_freq`.
  - [ ] `decay` → `amp_env.set_decay`.
  - [ ] `decay_slope` → `bank.set_q_profile` (modal-bank variants only).
  - [ ] `pulse_ms` → `excitation.set_pulse_ms` (modal-bank variants only).
  - [ ] `shimmer` → `mod_depth` cache (Cymbal2 / XorCymbal only).
- [ ] `tone`, `body_mix` are pure cached scalars; the existing
      `self.tone = p.get(params::tone)` etc. stay as-is (no
      downstream side effects to guard).
- [ ] Existing tests pass unmodified.
- [ ] New unit test on `Cymbal2`: build the harness, call
      `update_validated_parameters` ten times with identical params,
      verify the audio output for a 256-sample trigger run is
      bit-identical to a control run with a single
      `update_validated_parameters` call. (Equivalent guard test on
      one module suffices.)

## Notes

- Initial values from `prepare()` populate the cache, so the very
  first `update_validated_parameters` call after build still applies
  whatever the host sends (could be different from `prepare()`
  defaults — for safety, treat the cache as "needs first write" on
  the first call by initialising it to a sentinel like `f32::NAN`).
- Float `!=` comparison is fine here: the host sends quantised /
  cached param frames; identical inputs produce bit-identical
  values.
