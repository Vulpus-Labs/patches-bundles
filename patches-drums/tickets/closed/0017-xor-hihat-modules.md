---
id: "0017"
title: XorClosedHiHat + XorOpenHiHat modules — XOR generator with choke
priority: medium
created: 2026-05-19
epic: E004
---

## Summary

Add `XorClosedHiHat` and `XorOpenHiHat` — hi-hat voices structurally
identical to the existing
[ClosedHiHat / OpenHiHat](../../src/hihat.rs) with `MetallicTone`
replaced by `XorPairTone` (ticket 0015). Same envelopes, same
HP-noise mix via `tone`, same `choke` input on the open variant.
The flavour distinction is the generator's coarser inharmonic
texture.

Both modules live in one file (`xor_hihat.rs`) mirroring the
existing [hihat.rs](../../src/hihat.rs). Sister to the existing
hi-hats and (when E003 lands) `ClosedHiHat2` / `OpenHiHat2`. All
six coexist.

## Acceptance criteria

- [ ] New module `patches-drums/src/xor_hihat.rs` exporting both
  `XorClosedHiHat` and `XorOpenHiHat`.
- [ ] Module names: `XorClosedHiHat`, `XorOpenHiHat`. ABI version 1.
  Added to `patches_drums::register` and to the `export_modules!`
  invocation in [lib.rs](../../src/lib.rs#L41-L66).
- [ ] Ports and parameters identical to existing
  [ClosedHiHat](../../src/hihat.rs#L48) and
  [OpenHiHat](../../src/hihat.rs#L209) — same names, same ranges,
  same defaults. The contract is "drop-in alternative flavour".
- [ ] Process loops match [hihat.rs](../../src/hihat.rs) line-for-line
  with the substitution `MetallicTone` → `XorPairTone`. The
  `choke` handling on `XorOpenHiHat` is identical to
  [OpenHiHat:333–335](../../src/hihat.rs#L333-L335).
- [ ] Module rustdoc tables mirror the existing
  [hihat.rs:1–26](../../src/hihat.rs#L1-L26) and
  [hihat.rs:182–198](../../src/hihat.rs#L182-L198) blocks. Add a
  one-line module-level callout that these use the XOR-pair
  generator for a coarser, denser texture than the originals.
- [ ] Unit tests in `xor_hihat.rs` mirroring the existing hi-hat
  tests:
  - [ ] **XorClosedHiHat trigger produces output** — RMS > 0.01
    over 1000 samples.
  - [ ] **XorClosedHiHat short decay** — RMS at samples 8000–9000
    < 0.005.
  - [ ] **XorOpenHiHat long decay** — `decay = 0.4`, audible at
    9000, decayed at 45000.
  - [ ] **XorOpenHiHat choke silences** — same shape as
    [hihat.rs:418–448](../../src/hihat.rs#L418-L448).
  - [ ] **HF dominant** — HF/LF > 5.0 across 2048 samples on both
    modules.
  - [ ] **Spectrum differs from existing hi-hats** — average
    per-bin spectral magnitude difference between
    `XorClosedHiHat` and `ClosedHiHat` outputs (matched params,
    2048-sample run) > 0.005. Same test on the open pair.
  - [ ] **Velocity scales output** — half ≈ half, ±10%, on both.
- [ ] FFI manifest test in [lib.rs:88](../../src/lib.rs#L88) updated
  to include `"XorClosedHiHat"` and `"XorOpenHiHat"` in
  `EXPECTED_NAMES`.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.

## Notes

- Same anti-refactor stance as ticket 0016: don't extract a shared
  `hihat_shell` taking a generator trait. Two parallel
  implementations are clearer and small enough that the
  duplication is honest.
- If the existing `ClosedHiHat` / `OpenHiHat` implementations
  change, these modules follow. Keep them in lockstep with their
  `MetallicTone` siblings rather than diverging quietly.
- `XorPairTone::reset` (called via `trigger`) clears phases on
  every strike — the existing
  [`MetallicTone::trigger`](../../src/primitives/metallic.rs#L36)
  does the same. Same behavioural contract, same character.
- No shimmer modulation on hi-hats, matching the existing
  `MetallicTone`-based hi-hats (only `Cymbal` uses
  `tick_with_modulation`). If a later ticket adds shimmer to the
  hi-hats it should land on both flavours together.
