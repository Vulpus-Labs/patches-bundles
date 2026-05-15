---
id: "0002"
title: Async loader + FS watcher + double-buffer swap
priority: high
created: 2026-05-15
epic: E001
---

## Summary

Wrap the synchronous `load_dir_sync` from ticket 0001 in a loader thread
that owns the FS watcher, debounces filesystem events, and publishes
fresh arenas to the audio thread via an atomic-pointer double-buffer
swap. Cleanly join on `Drop`. The result is a `LiveStorage` building
block consumed by ticket 0003.

See [ADR 0001](../../adrs/0001-sampler-module.md) §"Async load +
double-buffer swap" and §"Filesystem observation".

## Acceptance criteria

- [ ] Add `notify` crate to `patches-drums` `Cargo.toml`. Approve
  dependency addition before merging.
- [ ] New module `sampler/live.rs` exposes:
  - [ ] `struct LiveStorage` holding two `Arc<SampleArena>`-equivalent
    cells and an `AtomicPtr<SampleArena>` (or `arc_swap::ArcSwap`
    if simpler — confirm before adding a second dep) for the front
    pointer. Whatever form chosen, the audio thread's read is
    lock-free and one `Acquire` load per tick.
  - [ ] `LiveStorage::new(dir: PathBuf, channels: usize, engine_sr:
    f32) -> Result<Self, ArenaLoadError>`: synchronously loads the
    initial arena into the front buffer, then spawns the loader
    thread to install the watcher and handle subsequent reloads.
  - [ ] `LiveStorage::front(&self) -> &SampleArena`: audio-thread
    accessor. Lock-free, no allocation, returns a `&` valid for the
    current tick.
  - [ ] `LiveStorage::observe_swap(&self) -> Option<SwapEvent>`:
    audio-thread-callable, returns `Some` the first time it observes
    a pointer change since the last call. Used by ticket 0004 to
    trigger the fade.
  - [ ] `impl Drop for LiveStorage`: flips a shutdown `AtomicBool`,
    drops the watcher, joins the loader thread.
- [ ] Loader thread:
  - [ ] Owns a `notify::RecommendedWatcher` for `dir`.
  - [ ] Coalesces events with a ~200 ms debounce window. Burst-saves
    from text editors collapse to one reload.
  - [ ] On debounce expiry, calls `load_dir_sync`, builds the new
    arena, swaps it into the back cell, atomic-stores the back
    pointer into the front-pointer atomic with `Release` ordering.
  - [ ] Owns the previous front arena after swap and drops it in its
    own thread context. Audio thread never runs a destructor.
  - [ ] Survives `load_dir_sync` errors: log the error, keep the
    current front arena live, wait for next event.
- [ ] Tests:
  - [ ] Determinism-friendly test: construct a `LiveStorage` over a
    tempdir; assert initial arena matches `load_dir_sync` of the
    same dir. Write a new file into the dir, wait up to 2 s for
    `observe_swap` to fire (use a poll-with-timeout helper; do not
    busy-loop). Confirm the new slot is populated.
  - [ ] Drop test: construct and immediately drop a `LiveStorage`,
    confirm the loader thread joins within 250 ms.
  - [ ] Error test: point at a non-existent dir, confirm `new`
    returns `ArenaLoadError::Io` without spawning a thread.
- [ ] No `unwrap()`/`expect()` in production paths.
- [ ] `just inner -p patches-drums` green.
- [ ] Document in the module's rustdoc that two non-audio threads
  are spawned (loader + notify's internal watcher).

## Notes

- `notify` spawns its own watcher thread internally; the loader
  thread is separate and drives the debounce timer + atomic swap.
- Triple-buffering was considered and rejected; ADR 0001 explains
  why double is sufficient.
- The atomic-pointer dance can be expressed with `arc_swap::ArcSwap`
  in twenty fewer lines than hand-rolled `AtomicPtr`. If a single
  extra small dep is acceptable, use it. Otherwise, hand-roll with
  a `Box::into_raw` / `Box::from_raw` pair guarded by acquire/release
  ordering.
- The FS-event test is the one place where this ticket's tests can
  be flaky; the 2 s ceiling is generous but not unbounded — if it
  proves flaky in CI, gate the test behind a feature flag and run
  it only in `just push` / `just smoke`.
