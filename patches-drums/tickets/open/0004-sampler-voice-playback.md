---
id: "0004"
title: Voice playback — pitch interp, loop modes, choke, swap-fade
priority: high
created: 2026-05-15
epic: E001
---

## Summary

Fill in the real-time core of `Sampler::tick()`: per-channel voice
playback with phase-increment pitch, linear interpolation, three loop
modes, immediate choke, and a ~1 ms exponential fade applied to active
voices when a `LiveStorage` swap is observed. After this ticket, the
module produces audio.

See [ADR 0001](../../adrs/0001-sampler-module.md) §"Playback
semantics" and §"Mid-playback swap".

## Acceptance criteria

- [ ] Per channel, the audio thread, in declared order:
  - [ ] Reads `trigger[i]`, `gate[i]`, `choke[i]`, `pitch[i]` from
    the cable pool.
  - [ ] On `trigger ↑`: set `pos = start[i] * len_i`, `playing =
    true`, `dir = +1`.
  - [ ] On `choke ↑` (whether or not playing): set `playing = false`.
  - [ ] Computes phase increment: `2_f64.powf(pitch[i] as f64) *
    (slot_native_rate / engine_rate) * dir as f64`. (Native rate
    stored per slot, defaulting to engine rate because `read_mono`
    already resamples — so the ratio is 1.0 in normal use; the
    factor is left in the expression for future per-slot SR
    overrides.)
  - [ ] Advances `pos` by `increment`; reads the two nearest sample
    frames and linear-interpolates.
  - [ ] Enforces loop / end logic per `loop_mode[i]`:
    - `off`: if `pos >= end[i] * len_i` (or `<= start[i] * len_i`
      while `dir = -1`), set `playing = false`.
    - `on`: if `pos >= loop_end[i] * len_i` and gate is high, wrap
      to `loop_start[i] * len_i`. On gate release while inside the
      loop, continue forward past `loop_end[i]` until `end[i]` is
      reached, then stop.
    - `alt`: if `pos >= loop_end[i] * len_i`, `dir = -1`; if `pos
      <= loop_start[i] * len_i`, `dir = +1`. Gate release behaves
      as in `on` — continue in current direction to the outer
      `end[i]` / `start[i]` boundary and stop.
  - [ ] Writes the interpolated, fade-scaled sample to `out[i]`.
  - [ ] Empty slots (zero-length range) always write silence.
- [ ] Swap-fade:
  - [ ] At the top of `tick`, call
    `live.observe_swap()`-equivalent. If `Some`, set a per-voice
    `fade_samples_left = round(engine_rate * 0.001)` (1 ms) for
    every voice with `playing = true`.
  - [ ] During fade, output is `sample * (fade_samples_left /
    fade_total)` (exponential preferred but linear acceptable for
    1 ms). When `fade_samples_left` hits 0, set `playing = false`.
  - [ ] Sealed-storage variant skips the swap check entirely (no
    swap can occur).
- [ ] Parameter clamping: each tick (or on `apply_unpacked_params`)
  ensure `start <= loop_start <= loop_end <= end` and all within
  `[0, 1]`. Out-of-range values are clamped, not rejected, to keep
  live-edited DSL kind. Document the clamp in the module rustdoc.
- [ ] Tests in `patches-drums/src/sampler/tests.rs`:
  - [ ] One-shot test: load a known short WAV in slot 0, fire a
    trigger, advance ticks until `playing = false`, assert output
    samples match the file within float tolerance.
  - [ ] Loop-on test: trigger with `loop_mode = on`, advance enough
    ticks to wrap the loop region twice, then release gate; assert
    playback continues to `end` and stops.
  - [ ] Loop-alt test: trigger with `loop_mode = alt`, assert
    direction flips at boundaries by inspecting `voice.dir` over
    many ticks.
  - [ ] Choke test: trigger then immediately choke; assert
    `playing` becomes false and `out` returns silence next tick.
  - [ ] Pitch test: trigger with `pitch = 1.0` (one octave up);
    assert phase advanced by ~2× expected over a known span.
  - [ ] Swap-fade test (sealed-mode mock): manually invoke the
    fade entry point and assert output decays to zero over the
    fade window, voice ends with `playing = false`.
- [ ] No audio-thread allocations. No `unwrap()`/`expect()` in
  production paths. `tick` has no `match` on storage variant per
  voice — branch is hoisted to once per tick (or zero, if the
  variants share a `&SampleArena` accessor).
- [ ] `just inner -p patches-drums` green.

## Notes

- Linear interpolation is enough for v1; SO little percussive
  audio survives high-frequency content above the original Nyquist
  that the cost of better interp isn't justified.
- Voice state is kept inside the module instance; no per-voice
  allocations. Adding voices means resizing once at build.
- The fade window length (1 ms) is a constant tuned by ear; not a
  parameter. If a user later wants longer crossfades on swap, that
  is a separate ticket.
- Phase-increment expression intentionally accommodates a future
  per-slot native-rate override even though `read_mono` already
  resamples to engine rate. Don't simplify it away.
- Voice ordering inside the tick is arbitrary; the module
  guarantees no inter-channel data dependency, so it remains
  parallelism-ready (see `CLAUDE.md` design desiderata).
