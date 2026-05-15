---
id: "0005"
title: Register, wire `auto_scan`, docs, golden tests
priority: medium
created: 2026-05-15
epic: E001
---

## Summary

Land the last mile: register `Sampler` with `patches_drums::register`,
add a working example patch, document the module in the bundle's
manual page, and lay down golden tests against the deterministic
`Storage::Sealed` path so future changes can't silently break
playback.

See [ADR 0001](../../adrs/0001-sampler-module.md). This ticket also
files the follow-up note for E148-driven migration of the `auto_scan`
flag source.

## Acceptance criteria

- [ ] `patches_drums::register` registers `Sampler` alongside the
  existing synthesised drums. Confirm via a registry-introspection
  test (or extend an existing one in `patches-drums/src/test_support.rs`).
- [ ] One new example under `patches-bundles/patches-drums/examples/`
  demonstrating the sampler — small kit of three or four short
  public-domain or generated WAVs, a sequencer, and a stereo mixdown.
  Confirm `cargo run -p patch_player -- <example>.patches` plays
  audio on a developer machine (manual verification noted in the
  ticket on close).
- [ ] Golden audio tests:
  - [ ] In `patches-integration-tests`, a new test that loads a
    fixed kit (small WAVs in `tests/fixtures/sampler/`), runs a
    deterministic trigger sequence, and compares output samples to
    a baseline file under a strict tolerance. Set
    `PATCHES_SAMPLER_AUTO_SCAN=0` for the test (or pass via the
    same mechanism the test harness already uses).
  - [ ] Test fixture WAVs are checked in (small) and licensed for
    redistribution (generated sine bursts are simplest).
- [ ] Module rustdoc on `Sampler` matches the module documentation
  standard in `CLAUDE.md` and reflects what shipped (sometimes
  diverges from ADR over the course of implementation; update the
  comment, not the ADR).
- [ ] Bundle-level manual entry (`docs/src/modules/sampler.md` in
  the main repo, or wherever the drum bundle's user-facing docs
  live) documents the module's structural params, ports, and
  parameters. If the source-of-truth comment is good, this can be a
  short note plus a link.
- [ ] Follow-up ticket filed against E148: "Migrate sampler
  `auto_scan` env-var read to `GlobalConfig::auto_scan_samples`."
  Reference both this epic and E148; do not block 0005 on it.
- [ ] All four tiers (`inner` / `commit` / `push` / `smoke`) green.
- [ ] Epic E001 moves to `epics/closed/`; tickets 0001-0005 move to
  `tickets/closed/`.

## Notes

- Goldens are byte-or-tolerance-comparable to a stored baseline.
  Use the existing audio-integrity scaffolding from
  `patches-integration-tests/tests/auto_conv_audio_integrity.rs`
  if its conventions transfer cleanly.
- The example patch is the user-facing artefact: aim for "load
  this, hear drums" rather than "demonstrate every feature".
- Don't forget the registration test — silent failure to register
  is the most common way a new module ships broken.
