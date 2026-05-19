---
id: "0020"
title: XorPairTone — remove dead branch, fast-path zero-depth modulation
priority: low
created: 2026-05-19
---

## Summary

Two small wins in [`XorPairTone::tick_with_modulation`](../../src/primitives/xor_pair_tone.rs#L69):

1. **Dead branch.** The increment is computed as
   `(base_inc + mod_offset).clamp(0.0, 0.499)` — guaranteed `≥ 0`.
   The follow-up `if *phase < 0.0 { *phase += 1.0; }` at
   [xor_pair_tone.rs:81-83](../../src/primitives/xor_pair_tone.rs#L81-L83)
   is therefore unreachable. Drop it.

2. **Zero-depth shortcut.** When `mod_depth == 0.0`, `mod_base = 0.0`
   and every per-partial `mod_offset = 0`, so the function behaves
   identically to the non-modulating `tick`. Currently the caller
   still pays one `fast_sine` per sample plus six clamps. Branch on
   `mod_depth == 0.0` at the top and dispatch to `self.tick()`.

[`XorCymbal::process`](../../src/xor_cymbal.rs#L176) unconditionally
calls `tick_with_modulation`; with shimmer=0 (default 0.2, but a
common preset choice) the savings apply per sample.

## Acceptance criteria

- [ ] [`XorPairTone::tick_with_modulation`](../../src/primitives/xor_pair_tone.rs#L69)
      early-returns `self.tick()` when `mod_depth == 0.0`.
- [ ] The `*phase < 0.0` branch at
      [xor_pair_tone.rs:81-83](../../src/primitives/xor_pair_tone.rs#L81-L83)
      is removed.
- [ ] Existing `XorPairTone` tests pass unmodified.
- [ ] Existing `XorCymbal` tests pass unmodified.
- [ ] New test: `tick_with_modulation(0.0, anything)` produces the
      same sample stream as `tick()` over 256 samples (exact
      equality, not approximate).

## Notes

- `XorCymbal::process` could additionally branch on
  `self.mod_depth > 0.0` and route to `xor.tick()` directly, saving
  one function call boundary. Optional follow-up; the primitive-side
  shortcut alone gets most of the win.
