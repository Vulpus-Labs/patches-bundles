---
id: "0019"
title: Cymbal2 / HiHat2 / Xor* — preserve HP-noise filter state across param updates
priority: medium
created: 2026-05-19
---

## Summary

[`Cymbal2::update_validated_parameters`](../../src/cymbal2.rs#L243),
[`ClosedHiHat2::update_validated_parameters`](../../src/hihat2.rs#L208),
[`OpenHiHat2::update_validated_parameters`](../../src/hihat2.rs#L403),
[`XorCymbal::update_validated_parameters`](../../src/xor_cymbal.rs#L148),
[`XorClosedHiHat::update_validated_parameters`](../../src/xor_hihat.rs#L139),
and [`XorOpenHiHat::update_validated_parameters`](../../src/xor_hihat.rs#L304)
all rebuild the HP-noise filter by reassigning
`self.hp_filter = SvfKernel::new_static(f, d)`. This **drops the
filter's `lp/bp` state to zero** on every param frame even if the
filter cutoff has not changed — the noise filter restarts from a
zero-state IIR transient whenever a host control surface tweaks any
unrelated parameter.

Audible effect: small click / spectral discontinuity on the noise
component each time any param changes during a sustained tail
(Cymbal2 8-s decay range is the worst case). With high-Q filtering
of broadband white noise the transient is short, but it is real and
unnecessary.

`SvfKernel` already has the right API:
[`set_static(f, d)`](../../patches/patches-dsp/src/svf/mod.rs#L144)
updates coefficients in place, preserving state.

## Acceptance criteria

- [ ] All six modules listed above call `self.hp_filter.set_static(f, d)`
      instead of `self.hp_filter = SvfKernel::new_static(f, d)`.
- [ ] Existing tests pass unmodified.
- [ ] New regression test on `Cymbal2`: trigger, let the noise tail
      ring, call `update_validated_parameters` mid-tail with the
      *same* `filter` value, and verify no audible discontinuity in
      the next 64 samples (sample-to-sample diff stays below a
      ceiling — e.g. 0.05). One module suffices; the others share
      the same code shape.

## Notes

- This is a state-correctness fix, not a performance one — but it
  belongs in the same review pass because the audible artifact is
  caused by the same "rebuild instead of update" pattern that motivates
  ticket 0022 (param-frame rebuild guards).
- A future ticket may convert `filter` to a CV-able input via the
  `wants_periodic` / `begin_ramp` pattern; this fix is upstream of
  that and remains correct under the ramped flow.
