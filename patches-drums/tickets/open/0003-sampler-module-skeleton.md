---
id: "0003"
title: Sampler module shape — descriptor, structural binding, ports
priority: high
created: 2026-05-15
epic: E001
---

## Summary

Stand up the `Sampler` module type: descriptor, structural-parameter
binding, per-channel input/output ports, parameter table. No playback
logic yet (that lands in ticket 0004) — this ticket gets the module
through `build()` and `apply_unpacked_params()` cleanly with both
storage variants instantiated.

See [ADR 0001](../../adrs/0001-sampler-module.md) §"Module shape" and
§"`auto_scan` global config".

## Acceptance criteria

- [ ] New `sampler::Sampler` module type, registered later in ticket
  0005.
- [ ] Static descriptor declares:
  - [ ] Structural params: `channels: int`, `dir: string`.
  - [ ] Per-channel inputs `trigger[i]` (trigger), `gate[i]` (gate),
    `choke[i]` (trigger), `pitch[i]` (CV). Names match ADR table.
  - [ ] Per-channel output `out[i]` (mono audio).
  - [ ] Per-channel parameters `start[i]`, `end[i]`, `loop_start[i]`,
    `loop_end[i]` (float, 0..1, defaults per ADR) and `loop_mode[i]`
    (enum off|on|alt, default off).
- [ ] `build()`:
  - [ ] Validates `channels` ≥ 1 and `dir` non-empty; returns
    `BuildError` with a useful message otherwise.
  - [ ] Reads the `auto_scan` flag from
    `PATCHES_SAMPLER_AUTO_SCAN` env var (default true). A later
    ticket against E148 swaps the source to `GlobalConfig`.
  - [ ] Constructs `Storage::Sealed { samples: SampleArena }` via
    `load_dir_sync` when `auto_scan` is false. Failures propagate as
    `BuildError::Custom` with module name `"Sampler"`.
  - [ ] Constructs `Storage::Live { storage: LiveStorage }` when
    `auto_scan` is true. The initial sync load inside
    `LiveStorage::new` is the cost paid at build time; subsequent
    reloads are async.
  - [ ] Returns immediately; never blocks the build call on more than
    one synchronous decode pass.
- [ ] `apply_unpacked_params` accepts the per-channel parameter table
  and stores it in a `[ChannelParams; ...]`-equivalent layout. No
  use of these values yet beyond persisting them.
- [ ] Voice state: `Vec<Voice>` of length `channels`, each `Voice
  { pos: f64, playing: bool, dir: i8 }`. All zeroed/false at build.
- [ ] No allocation on the audio path. `tick()` is a no-op stub
  returning silence; real playback lands in ticket 0004.
- [ ] No `unwrap()`/`expect()` in production paths.
- [ ] Module-level doc comment follows the module documentation
  standard in `CLAUDE.md` (Inputs / Outputs / Parameters tables).
  Port-name strings in the comment match the descriptor exactly.
- [ ] `just inner -p patches-drums` green; module compiles and
  registers (in a local test harness — full registry wiring is
  ticket 0005).

## Notes

- For descriptor / structural-binding patterns, mirror
  `patches-bundles/patches-fft-bundle/src/convolution_reverb/mod.rs`
  which has both a `structural` string param and a structural-style
  shape; the difference is per-channel arrays of ports and params.
- For per-channel ports, mirror an existing poly module — `Op` in
  `patches-modules/src/poly_op.rs` or `PolyAdsr` in
  `patches-modules/src/poly_adsr.rs`. Note the static descriptor
  conventions (zero-cost descriptors per `CLAUDE.md` design
  desiderata).
- Two storage variants prevent a per-tick branch on a runtime "is
  live" flag (ADR §"`auto_scan` global config"). Implement them as a
  small `enum Storage { Sealed { ... }, Live { ... } }` with an
  inline accessor returning `&SampleArena` for the current tick.
- Validation messages should name the structural param at fault
  (`"channels must be >= 1"`, `"dir must not be empty"`).
