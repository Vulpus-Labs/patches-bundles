---
id: E001
title: Sampler module for the drums bundle
status: open
created: 2026-05-15
adr: 0001
---

## Goal

Add a `Sampler` module to `patches-drums` that loads a directory of
audio files, exposes one trigger-and-output pair per file, and reloads
asynchronously when the directory changes — without ever allocating or
blocking on the audio thread. See
[ADR 0001](../adrs/0001-sampler-module.md).

The module's job description, in one line: be a sample-playback
counterpart to the synthesised drums in this bundle, usable live and
polite to its host.

## Scope

- New `sampler` submodule in `patches-drums/src/`, registered by
  `patches_drums::register` alongside the existing synthesised drums.
- Structural params: `channels: int`, `dir: string`. Files indexed by
  numeric prefix `NN_*.{wav,aiff,aif}`. Stereo files summed to mono.
- Per-channel runtime ports: `trigger[i]`, `gate[i]`, `choke[i]`,
  `pitch[i]` in; `out[i]` audio out.
- Per-channel parameters: `start[i]`, `end[i]`, `loop_start[i]`,
  `loop_end[i]`, `loop_mode[i]` (enum `off`/`on`/`alt`).
- WAV decode + resample reuses `patches_io::read_mono`.
- Indexed-range arena: one `Vec<f32>` + `[Range<usize>; channels]`
  per loaded kit.
- Two storage variants — `Storage::Sealed` (sync load, single
  buffer, no watcher) and `Storage::Live` (async loader thread,
  double-buffer, FS watcher) — selected by host `auto_scan` flag.
- Async path: loader thread owns the FS watcher (`notify` crate) and
  the back buffer. Atomic-pointer swap publishes new arenas. ~1 ms
  exponential fade on active voices when a swap is observed.
- One new dependency: `notify` crate in `patches-drums`. Approve
  before adding (per `CLAUDE.md`).
- Tests: synth-style unit tests on the playback engine, golden tests
  for one-shot and looped playback, integration test that
  exercises the `Storage::Sealed` path end-to-end. The `Storage::Live`
  path gets a determinism-aware test using a tempdir + manual swap
  trigger (no real FS-event timing dependency).

## Out of scope

- Velocity layers, pitch zones, round-robin selection.
- Mipmap / bandlimited octave layers for upward pitch shifts.
- Time-stretching, formant preservation, anything elastique-like.
- Stereo output (single stereo file → stereo out). v1 sums to mono.
- CV-variable `start`/`end`/`loop_*`. Parameters only in v1.
- Per-slot sample-rate overrides; engine SR is assumed.
- Migration of v1's `PATCHES_SAMPLER_AUTO_SCAN` env-var read to
  `GlobalConfig::auto_scan_samples`. Tracked as a follow-up against
  E148, not in this epic.

## Tickets

- [0001 — Sample arena: dir scan, WAV decode, resample, indexed-range layout](../tickets/open/0001-sampler-arena.md)
- [0002 — Async loader + FS watcher + double-buffer swap](../tickets/open/0002-sampler-async-loader.md)
- [0003 — `Sampler` module shape: descriptor, structural binding, ports](../tickets/open/0003-sampler-module-skeleton.md)
- [0004 — Voice playback: pitch interp, loop modes, choke, swap-fade](../tickets/open/0004-sampler-voice-playback.md)
- [0005 — Register, wire `auto_scan`, docs, golden tests](../tickets/open/0005-sampler-wire-and-docs.md)

## Acceptance

- A `Sampler` module is registered by `patches_drums::register` and
  appears in `default_registry`.
- A `.patches` file using `Sampler` with a directory of small WAVs
  loads, runs, and produces audio for each triggered slot.
- Building the module returns in <10 ms regardless of how long
  decoding takes; slots become audible as decoding completes.
- With `auto_scan` enabled, dropping a new `NN_*.wav` into the
  directory causes that slot to become audible after a sub-second
  delay, without any DSL reload.
- With `auto_scan` disabled, the module loads once at build, holds
  no watcher thread, and is bit-identical across runs (golden tests
  pass).
- No allocations on the audio thread under release-mode profiling
  (existing harness in `patches-modules` or a new equivalent).
- All four tiers (`inner` / `commit` / `push` / `smoke`) green.
