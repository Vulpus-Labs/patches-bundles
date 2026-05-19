---
id: E004
title: XOR-pair metal voices — XorCymbal, XorClosedHiHat, XorOpenHiHat
status: closed
created: 2026-05-19
adr: 0002
---

## Goal

Add `XorCymbal`, `XorClosedHiHat`, `XorOpenHiHat` — three new metal
voices built around an inharmonic generator formed by pairing six
square oscillators into three pairs, multiplying each pair (XOR of
bipolar squares is equivalent to multiplication), and summing the
three pair outputs. The result is a denser, coarser inharmonic
signal than the existing `MetallicTone`'s direct sum of six squares,
because the intermod products of each pair contain energy at
frequencies neither input oscillator carried alone. A specific,
distinctive flavour — not a replacement for the existing metal
voices.

These coexist with the existing `Cymbal` / `ClosedHiHat` /
`OpenHiHat` and the modal variants from [E003](E003-modal-metal-voices.md).

## Scope

- New primitive `XorPairTone` in
  `patches-drums/src/primitives/xor_pair_tone.rs`. Six square
  oscillators arranged as three pairs; each pair's outputs are
  multiplied, the three products are summed and normalised. Same API
  shape as `MetallicTone` (`new`, `set_frequency`, `trigger`,
  `tick`, `tick_with_modulation`) so the metal voice shells differ
  from their existing counterparts only in the generator swap.
- New module `XorCymbal` in `patches-drums/src/xor_cymbal.rs`.
  Structurally identical to existing `Cymbal` with `MetallicTone`
  replaced by `XorPairTone`. Same envelope, same HP-noise mix, same
  shimmer LFO.
- New modules `XorClosedHiHat` + `XorOpenHiHat` in
  `patches-drums/src/xor_hihat.rs`. Structurally identical to
  existing `ClosedHiHat` / `OpenHiHat` with the generator swap.
- All three modules registered by `patches_drums::register` and
  exported through `export_modules!`. ABI version 1.
- Per-module unit tests using the existing
  [test_support](../src/test_support.rs) helpers.

## Out of scope

- Replacing or deprecating the existing `Cymbal`, `ClosedHiHat`,
  `OpenHiHat`. The XOR voices are a distinct flavour offered
  alongside.
- Re-using the `XorPairTone` primitive for non-metal voices in this
  epic. Possible follow-up; not in scope.
- Modal-bank-style per-partial decay. The XOR voices share the
  existing modules' single outer envelope — that's part of what makes
  them the "rough metal" flavour rather than the "realistic ring"
  flavour.
- Exposing the per-pair frequency relationships as user parameters.
  v1 ships with fixed pair-ratio assignments; an "inharmonicity"
  knob is a possible follow-up.

## Tickets

- [0015 — `XorPairTone` primitive: three XOR-pair squares summed](../tickets/closed/0015-xor-pair-tone-primitive.md)
- [0016 — `XorCymbal` module: XOR generator with HP-noise crash and shimmer](../tickets/closed/0016-xor-cymbal-module.md)
- [0017 — `XorClosedHiHat` + `XorOpenHiHat` modules: XOR generator with choke](../tickets/closed/0017-xor-hihat-modules.md)

## Acceptance

- `XorCymbal`, `XorClosedHiHat`, `XorOpenHiHat` are registered by
  `patches_drums::register` and appear in the FFI manifest with ABI
  version 1.
- A `.patches` file using each module produces audible, decaying
  metal voice on trigger.
- The XOR voices have measurably different spectra from their
  `MetallicTone`-based counterparts at matched parameters — energy
  distribution in the upper bands shows the intermod-product
  contribution. A regression test pins one rough spectral
  characteristic so future generator tweaks are caught.
- `XorOpenHiHat`'s `choke` input silences the voice within ≤ 50 ms.
- No allocations on the audio thread under release-mode profiling.
- All four tiers (`inner` / `commit` / `push` / `smoke`) green.
