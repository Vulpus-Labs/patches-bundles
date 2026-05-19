---
id: E003
title: Modal resonator bank metal voices — Cymbal2, ClosedHiHat2, OpenHiHat2
status: closed
created: 2026-05-19
adr: 0002
---

## Goal

Add `Cymbal2`, `ClosedHiHat2`, and `OpenHiHat2` — three new metal
voices built around a bank of parallel high-Q resonators (one per
inharmonic partial) rather than the summed-square `MetallicTone`
generator used by the existing `Cymbal` / `ClosedHiHat` /
`OpenHiHat`. Each partial rings with its own Q so the ensemble
decays inhomogeneously (high partials fade first, low partials linger)
and adjacent partials beat against each other in the tail. These
coexist with the existing modules rather than replacing them.

The current generator has all partials sharing one outer envelope.
That coupling caps the realism: real metals decay per-mode, and the
beating between near-ratio partials is half of what gives a cymbal
its complex tail. A resonator bank gives both for free.

Sister to the bridged-T family established in
[ADR 0002](../adrs/0002-bridged-t-resonator-family.md). Reuses
`BridgedT` and `Excitation` from E002.

## Scope

- New primitive `ModalBank` in
  `patches-drums/src/primitives/modal_bank.rs`. Six `BridgedT`
  instances at configurable inharmonic ratios, per-partial Q,
  per-partial output gain, single shared excitation tap on the input.
- New module `Cymbal2` in `patches-drums/src/cymbal2.rs`. Modal bank
  at cymbal-like ratios with long decay slope, parallel low body
  resonator for the gong-weight under the crash, HP-noise crash blend
  matching the existing `Cymbal`'s `tone` mix shape, shimmer LFO on
  partial frequencies for the slow modulation.
- New modules `ClosedHiHat2` + `OpenHiHat2` in
  `patches-drums/src/hihat2.rs`. Modal bank at hi-hat-like ratios.
  Closed = short decay. Open = long decay + `choke` input mirroring
  the existing `OpenHiHat`.
- All three modules registered by `patches_drums::register` and
  exported through `export_modules!`. ABI version 1.
- Per-module unit tests using the existing
  [test_support](../src/test_support.rs) helpers (`band_energy`,
  `magnitude_spectrum`, `dominant_bin`, `windowed_rms`).

## Out of scope

- Replacing or deprecating the existing `Cymbal`, `ClosedHiHat`,
  `OpenHiHat`. Two families coexist.
- Per-partial CV control. Realtime params shape the bank globally
  via decay-slope / inharmonicity / pitch; no individual per-partial
  inputs.
- Modal bank with > 6 partials. Six matches the existing inharmonic
  ratio set and keeps CPU bounded. Larger banks are a follow-up.
- Wave-digital-filter or in-loop-nonlinearity treatment of the
  resonators. `BridgedT`'s output saturator is enough.
- A modal `Snare2` body. Tracked separately if and when it lands.

## Tickets

- [0012 — `ModalBank` primitive: parallel BridgedT bank with per-partial Q](../tickets/closed/0012-modal-bank-primitive.md)
- [0013 — `Cymbal2` module: modal bank with body resonator and shimmer](../tickets/closed/0013-cymbal2-module.md)
- [0014 — `ClosedHiHat2` + `OpenHiHat2` modules: modal bank with choke](../tickets/closed/0014-hihat2-modules.md)

## Acceptance

- `Cymbal2`, `ClosedHiHat2`, `OpenHiHat2` are registered by
  `patches_drums::register` and appear in the FFI manifest with ABI
  version 1.
- A `.patches` file using each module produces audible, decaying
  metal voice on trigger.
- `Cymbal2`'s tail spectrum shifts darker over time: HF / total
  energy ratio at sample 4096 is smaller than at sample 0–2048
  (high partials decay first).
- `OpenHiHat2`'s `choke` input silences the voice within ≤ 50 ms
  of a rising edge, matching the existing `OpenHiHat`.
- No allocations on the audio thread under release-mode profiling.
- All four tiers (`inner` / `commit` / `push` / `smoke`) green.
