---
id: "0016"
title: XorCymbal module — XOR generator with HP-noise crash and shimmer
priority: medium
created: 2026-05-19
epic: E004
---

## Summary

Add `XorCymbal`, a cymbal voice structurally identical to the
existing [Cymbal](../../src/cymbal.rs) with `MetallicTone` replaced
by `XorPairTone` (ticket 0015). Same outer envelope, same HP-noise
mix via `tone`, same shimmer LFO. The point is the generator's
specific flavour — denser, coarser inharmonic spectrum from the
XOR-pair intermod products — not a paradigm shift in architecture.

Sister to existing `Cymbal` and (when E003 lands) `Cymbal2`. All
three coexist.

## Acceptance criteria

- [ ] New module `patches-drums/src/xor_cymbal.rs`.
- [ ] Module name: `XorCymbal`. ABI version 1. Added to
  `patches_drums::register` and to the `export_modules!` invocation
  in [lib.rs](../../src/lib.rs#L41-L66).
- [ ] Ports identical to [Cymbal](../../src/cymbal.rs#L78-L84):
  - [ ] `trigger` (mono, trigger).
  - [ ] `velocity` (mono, audio).
  - [ ] `out` (mono, audio).
- [ ] Realtime parameters identical to
  [Cymbal](../../src/cymbal.rs#L86-L106) — `pitch`, `decay`, `tone`,
  `filter`, `shimmer`. Defaults matching the existing module:
  `pitch = 600`, `decay = 2.0`, `tone = 0.5`, `filter = 6000`,
  `shimmer = 0.2`. Ranges identical too — the contract is "drop-in
  alternative flavour".
- [ ] No structural parameters. The XOR generator's pair-grouping
  is internal to the primitive.
- [ ] Process loop matches [Cymbal::process](../../src/cymbal.rs#L175-L207)
  exactly, with the single substitution `MetallicTone` → `XorPairTone`
  and `metallic.tick_with_modulation` → `xor.tick_with_modulation`.
- [ ] Module rustdoc table mirrors the existing
  [Cymbal:1–28](../../src/cymbal.rs#L1-L28) block. Add a one-line
  callout in module-level rustdoc that this variant uses the
  XOR-pair generator for a coarser, denser inharmonic texture
  versus the existing `Cymbal`.
- [ ] Unit tests in `xor_cymbal.rs` (mirror existing
  [cymbal.rs tests:212–322](../../src/cymbal.rs#L212-L322)):
  - [ ] **Trigger produces output** — RMS > 0.001 over 5000
    samples.
  - [ ] **Long decay** — at `decay = 4.0`, audible at 1 s.
  - [ ] **Shimmer modulates output** — `shimmer = 1.0` differs
    from `shimmer = 0.0` by average diff > 0.001.
  - [ ] **HF dominant** — total HF / LF > 4.0 across 4096 samples.
  - [ ] **Envelope peak is early** — peak windowed-RMS in first
    quarter of 8192-sample run.
  - [ ] **Spectrum differs from Cymbal** — at matched parameters,
    average per-bin spectral magnitude difference between
    `XorCymbal` and `Cymbal` outputs is > 0.005. Pins the
    flavour-distinction property as a regression test so a future
    primitive tweak that accidentally makes the two converge gets
    caught.
  - [ ] **Velocity scales output** — half ≈ half, ±10%.
- [ ] FFI manifest test in [lib.rs:88](../../src/lib.rs#L88) updated
  to include `"XorCymbal"` in `EXPECTED_NAMES`.
- [ ] No `unwrap()` or `expect()` in the new code.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.

## Notes

- This module is intentionally the smallest possible delta from
  `Cymbal`. If `Cymbal`'s implementation changes (e.g. a noise
  filter tweak), this module should follow it — keep them parallel
  in implementation as well as in interface.
- No body resonator here. `Cymbal2` (E003) adds the body-resonator
  feature; this module belongs to the XOR-flavour family which is
  about generator texture, not architectural enrichment.
- Refactor temptation: factor a `cymbal_shell` helper that takes a
  generator trait. Resist for now. Two parallel implementations are
  easier to read than one polymorphic shell, and the
  cymbal-as-template pattern hasn't proven itself yet across enough
  variants to warrant the abstraction.
