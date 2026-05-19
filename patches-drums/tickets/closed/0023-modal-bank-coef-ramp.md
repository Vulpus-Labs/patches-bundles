---
id: "0023"
title: ModalBank / TptSvf — adopt CoefRamp for block-rate ramped coefficients
priority: medium
created: 2026-05-19
resolution: wontfix
resolved: 2026-05-19
---

## Resolution: wontfix

After ticket [0018](../closed/0018-fast-tan-tpt-svf.md) landed
(`fast_tan_small` in `TptSvf`), the per-sample coefficient
computation dropped from ~40 cyc (libm `tan`) to ~5 cyc. The SVF
recurrence itself is ~8 cyc/sample. Total per-tick cost is now
~13 cyc; block-rate ramping would replace the 5-cyc `fast_tan_small`
with 5 lerps for the same total — no perf win.

Self-FM (Kick2, Tom2) needs the per-sample coefficient path
regardless, so the ramp wouldn't help those voices at all.

The remaining argument was workspace-wide consistency with the
`CoefRamp` pattern used by Chamberlin SVF / biquad / ladder kernels.
Soft reason; not worth the API refactor and ADR work given zero
audible payoff.

**Re-open trigger**: if a future Cymbal2 stress test (e.g. 8+
polyphonic voices with shimmer at 8-s decay) shows per-sample
coefficient cost as a meaningful share of the profile, revisit.

---

## Summary

[`BridgedT::tick`](../../src/primitives/bridged_t.rs#L83) recomputes
the TPT SVF coefficients every sample by calling
[`TptSvf::set_f_q`](../../src/primitives/tpt_svf.rs#L66). With
[`ModalBank`](../../src/primitives/modal_bank.rs) running six
resonators, [`Cymbal2`](../../src/cymbal2.rs) running a body
resonator on top, and each call performing one `tan` (or one
`fast_tan_small` after ticket 0018) plus a divide and a small
chain of muls, the per-sample arithmetic per voice is non-trivial.

The patches workspace already has a well-trodden answer to this
pattern for Chamberlin SVF / biquad / ladder kernels:
[`CoefRamp<K>` / `CoefTargets<K>`](../../../patches/patches-dsp/src/coef_ramp.rs)
(documented in patches ADR 0050). Kernels store hot active+delta
coefficients and cold targets; the containing module declares
`wants_periodic() = true` and calls
[`begin_ramp`](../../../patches/patches-dsp/src/svf/mod.rs#L160) at
each periodic-update boundary. Per-sample `tick` is a pure linear
recurrence plus a `coefs.advance()`.

For the shimmer-rate modulation in `Cymbal2`, block-rate coefficient
updates are far above the modulation bandwidth (~3 Hz LFO) and the
audible result is indistinguishable. For HiHat2 (no modulation at
all), coefficients are constant between param frames and the ramp
collapses to `set_static`. **Self-FM** in [`Kick2`](../../src/kick2.rs)
and [`Tom2`](../../src/tom2.rs) (`lp_prev` → `fm_offset`) *needs*
the per-sample coefficient path because the pitch-droop character
depends on per-sample tracking — those callers keep a `tick_fm`
variant.

This is the architectural pass that the cheaper tickets (0018, 0019,
0020, 0021, 0022) deliberately leave room for. They land first; this
ticket builds on the cleaner state.

## Acceptance criteria

- [ ] [`TptSvf`](../../src/primitives/tpt_svf.rs) gains:
  - `coefs: CoefRamp<5>` for `(g, k, a1, a2, a3)`,
  - `targets: CoefTargets<5>`,
  - `set_static(fc_hz, q)` — snap coefs, zero deltas,
  - `begin_ramp(fc_hz, q, interval_recip)` — snap-on-begin,
    compute deltas, apply `g_max` clamp to both snapped active
    and new target,
  - `tick(x) -> (lp, hp, bp)` — pure recurrence + `coefs.advance()`,
  - `tick_fm(x, fm_offset) -> (lp, hp, bp)` — retains the
    per-sample `set_f_q` semantics for self-FM callers.
- [ ] [`BridgedT`](../../src/primitives/bridged_t.rs) gains a
      `tick(x)` static path and keeps `tick_fm(x, fm_offset)` for
      self-FM. (Or: change the existing `tick(x, fm_offset)` to
      branch on `fm_offset == 0.0`. Pick the option that produces
      the smaller call-site diff.)
- [ ] [`ModalBank`](../../src/primitives/modal_bank.rs) routes its
      `tick` through the static path and its `tick_with_modulation`
      through `begin_ramp`-per-resonator at periodic-update
      boundaries, *not* the per-sample FM path.
- [ ] [`Cymbal2`](../../src/cymbal2.rs),
      [`ClosedHiHat2`](../../src/hihat2.rs),
      [`OpenHiHat2`](../../src/hihat2.rs) declare
      `wants_periodic() = true` and implement `periodic_update` that
      computes per-resonator targets (including the shimmer LFO
      sample, evaluated once per update interval) and calls
      `bank.begin_ramp(...)`.
- [ ] [`Kick2`](../../src/kick2.rs) and [`Tom2`](../../src/tom2.rs)
      switch their direct `BridgedT::tick` calls to `tick_fm` (or
      equivalent) and continue to recompute coefficients per sample.
- [ ] ADR addition under
      [`patches-drums/adrs/`](../../adrs/): "TPT SVF coefficient
      ramping" — document the per-sample vs block-rate split, why
      self-FM stays per-sample, and how the small-angle clamp
      participates in `begin_ramp`. Cross-reference patches ADR
      0050.
- [ ] All existing tests for `BridgedT`, `ModalBank`, `Cymbal2`,
      `ClosedHiHat2`, `OpenHiHat2`, `Kick2`, `Tom2` pass.
- [ ] New surface test on `ModalBank`: shimmer-modulated output
      from the block-rate path differs from the static-coef path by
      ≤ 1e-3 average per-sample magnitude over 4096 samples
      (validates the block-rate approximation is audibly equivalent).
- [ ] `cargo test -p patches-drums` green.

## Notes

- Stability clamp: mirror the `stability_clamp` pattern from
  [`patches_dsp::svf::SvfKernel::begin_ramp`](../../../patches/patches-dsp/src/svf/mod.rs#L160-L170)
  — apply `g.min(G_MAX)` to both the new target *and* the snapped
  active (computed from previous target), so the ramp never crosses
  the stability boundary even during interpolation.
- `interval_recip` is cached in each module from
  `audio_environment.periodic_update_interval` in `prepare()`,
  matching the rest of the workspace.
- This ticket is the *only* one in the 0018–0023 sequence that
  changes module-level scheduling (`wants_periodic`). The earlier
  tickets are pure local changes.
