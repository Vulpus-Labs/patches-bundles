---
id: "0001"
title: Sample arena — dir scan, WAV decode, resample, indexed-range layout
priority: high
created: 2026-05-15
epic: E001
---

## Summary

Build the loader-side primitive that takes a directory plus engine sample
rate and returns a single indexed-range arena ready to hand to the audio
thread. No threading, no FS watcher, no module wiring yet — just the data
structure plus the synchronous load path that everything else builds on.

See [ADR 0001](../../adrs/0001-sampler-module.md) §"Sample storage:
indexed-range arena".

## Acceptance criteria

- [ ] New module `patches-drums/src/sampler/arena.rs` (or
  `sampler/mod.rs` + private submodule, as the implementation evolves)
  exposes:
  - [ ] `struct SampleArena { samples: Vec<f32>, slots: Box<[Range<usize>]> }`
  - [ ] `SampleArena::empty(channels: usize) -> Self`
  - [ ] `SampleArena::slot(&self, i: usize) -> &[f32]` returns the
    slot's audio (empty slice for missing slots).
- [ ] Loader entry point `load_dir_sync(dir: &Path, channels: usize,
  engine_sr: f32) -> Result<SampleArena, ArenaLoadError>`:
  - [ ] Reads `dir` and selects files matching `NN_*.{wav,aiff,aif}`
    case-insensitive on extension.
  - [ ] Parses leading `NN` as the slot index; out-of-range or
    duplicate indices return a typed error.
  - [ ] Calls `patches_io::read_mono(path, engine_sr as f64)` per file;
    stereo files are summed by `read_mono` already.
  - [ ] Packs decoded samples into the arena contiguously and records
    each slot's `Range<usize>`. Missing slots get a zero-length range.
- [ ] `ArenaLoadError` variants: `Io(io::Error)`, `Decode { path,
  message }`, `BadPrefix { name, reason }`, `DuplicatePrefix { idx,
  first, second }`, `IndexOutOfRange { idx, channels }`.
- [ ] Unit tests cover:
  - [ ] Happy path: tmpdir with three small WAVs at prefixes 00, 02,
    05 with `channels = 6` produces an arena with three populated and
    three empty slots, samples preserved within float tolerance.
  - [ ] Missing-dir error is `ArenaLoadError::Io`.
  - [ ] Bad prefix (`foo_kick.wav`) is `BadPrefix`.
  - [ ] Duplicate prefix is `DuplicatePrefix`.
  - [ ] Out-of-range prefix (`20_kick.wav` with `channels = 8`) is
    `IndexOutOfRange`.
- [ ] No `unwrap()` or `expect()` in the new code (per `CLAUDE.md`).
- [ ] `just inner -p patches-drums` green.

## Notes

- `patches_io::read_mono` already handles WAV/AIFF decode + resample;
  the convolution reverb's `ir_path` path is the template
  (`patches-bundles/patches-fft-bundle/src/convolution_reverb/mod.rs:155`).
- No async or watcher concerns yet — those land in ticket 0002 and
  build on this synchronous primitive.
- The arena's `slots` field is `Box<[Range<usize>]>` rather than
  `Vec` to make its fixed length structural.
- Stereo handling: out of scope per ADR §"Out of scope". `read_mono`
  already collapses stereo input.
- Filename regex: prefer a small hand-written parser over the `regex`
  crate to avoid a new dependency for one tiny job.
