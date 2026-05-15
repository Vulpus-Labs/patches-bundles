# ADR 0001 — Sampler module with async dir-watched loading

## Status

Proposed (2026-05-15).

## Context

The drums bundle today is entirely synthesis-based (kick, snare, clap,
hihat, tom, cymbal, claves). For sample-based percussion and one-shots
we want a `Sampler` module that loads audio files from a directory and
exposes one trigger-and-output pair per file.

Three design pressures shape the module:

- **DAW politeness.** Loading and decoding sixteen WAV files
  synchronously on a structural rebuild can block the host's UI
  thread for hundreds of milliseconds. Production-quality DAW
  residents load asynchronously.
- **Liveness.** A live-coding workflow lets users drop files into the
  sample directory mid-set and hear them appear without re-saving the
  patch. The host should observe the filesystem and reload
  transparently.
- **Real-time safety.** The audio thread must never allocate, block,
  or read disk. Whatever liveness mechanism we add must respect ADR
  0009 (audio-thread-owned module pool) and the audio-engine
  conventions in `CLAUDE.md`.

`patches-fft-bundle/convolution_reverb` already uses
`patches_io::read_mono(path, sample_rate)` to load and resample WAV
files at structural-resolution time. That gives us the decode path for
free; what is new is the async + dir-watching machinery.

## Decision

### Module shape

One module type: `Sampler`, registered by `patches-drums`.

**Structural parameters** (resolved at build time, fixed for instance
lifetime):

| Name | Type | Description |
|------|------|-------------|
| `channels` | int | Number of slots, ≥1. |
| `dir` | string | Directory of sample files. |

Files are picked up by numeric prefix `NN_*.{wav,aiff,aif}`, where
`NN` is the slot index `0..channels-1`. Gaps allowed — missing
prefixes leave silent slots. Stereo files are summed to mono on load
(out-of-scope for v1; revisit when a poly-stereo variant appears).

**Per-channel runtime ports** (indexed `port[i]`, `i in 0..channels-1`):

| Port | Direction | Kind | Description |
|------|-----------|------|-------------|
| `trigger[i]` | in | trigger | Rising edge → restart playback from `start[i]`. |
| `gate[i]` | in | gate | While high, allow looping; release lets playback continue to `end[i]`. |
| `choke[i]` | in | trigger | Rising edge → stop immediately. |
| `pitch[i]` | in | cv | 1V/oct phase-increment multiplier. |
| `out[i]` | out | audio | Mono playback. |

**Per-channel parameters** (set in the DSL, not CV-variable):

| Name | Type | Range | Default | Description |
|------|------|-------|---------|-------------|
| `start[i]` | float | 0..1 | 0 | Sample-relative playback start. |
| `end[i]` | float | 0..1 | 1 | Sample-relative end. |
| `loop_start[i]` | float | 0..1 | 0 | Loop-region start. |
| `loop_end[i]` | float | 0..1 | 1 | Loop-region end. |
| `loop_mode[i]` | enum | `off`\|`on`\|`alt` | `off` | None, forward loop, or ping-pong. |

Start/end and loop endpoints stay parameters rather than CV inputs:
the drum use case is static slice-and-trigger, and CV reads cost 64
extra reads per tick at `channels=16` to no musical end. Pitch
remains CV because tuning per hit is a real workflow.

Playback semantics:

- `trigger[i] ↑` sets `pos = start[i] * len`, `playing = true`, `dir = +1`.
- `loop_mode = off`: play forward to `end`, then stop.
- `loop_mode = on`: at `loop_end`, jump to `loop_start` while gate is
  high; on gate release, continue forward to `end` and stop.
- `loop_mode = alt`: flip `dir` at `loop_start` and `loop_end` while
  gate is high; on gate release, continue in current direction to
  `end`/`start` and stop.
- `choke[i] ↑`: stop immediately, regardless of mode.
- Inter-sample positions are linear-interpolated. Upward pitch shifts
  alias above Nyquist; no mipmap (intentional — drums are rarely
  tuned far up, and the cost was deemed not worth it).

Per-channel voice state is one `f64` position, one `bool` playing,
one `i8` direction. Sixteen channels at sixteen bytes each is
trivial.

### Sample storage: indexed-range arena

Instead of `Vec<Option<Vec<f32>>>` (one heap allocation per slot), a
loaded kit is **one** `Vec<f32>` plus `[Range<usize>; channels]`
indexing into it. Benefits:

- One allocation and one deallocation per kit, not `channels` of each.
- Contiguous in memory — cache-friendly for the audio thread.
- Cheap to swap (one pointer, one ranges array).

Empty slots get a zero-length range.

### Async load + double-buffer swap

A loader thread owned by the module instance:

1. Decodes the directory off the audio thread using
   `patches_io::read_mono`.
2. Builds a fresh arena into a back buffer.
3. Publishes the back buffer via an atomic pointer swap.
4. Reclaims the previous front for the next reload.

The audio thread, at the top of each tick (or in
`Module::periodic_update`), `Acquire`-loads the front pointer and
reads from it for the rest of the tick. Reads are lock-free.

**Double-buffer, not triple.** The audio thread reads every tick
(sub-millisecond); the loader is disk-bound (milliseconds to
seconds). The producer is always slower than the consumer, so the
prior front buffer is always free for the next load. Triple would
waste a third of the kit's memory for a race that cannot occur in
practice.

**Drop discipline.** Buffer ownership returns to the loader thread on
swap; the loader's next cycle drops the old arena. The audio thread
never executes a destructor.

**Mid-playback swap.** If a voice is playing when the front pointer
changes, the slot's underlying audio may differ (file replaced,
range shifted, slot now empty). To avoid clicks, the module applies
a ~1 ms exponential fade to all active voices on swap detection.
This is cheap (per-voice float multiply during fade window) and
removes the worst audible artefact without needing per-slot
generation counters or `Arc`-based retention.

### Filesystem observation

The loader thread also owns the FS watcher. We use the `notify`
crate (real OS watchers: inotify on Linux, FSEvents on macOS,
ReadDirectoryChangesW on Windows). Events are debounced ~200 ms to
collapse editor save-temp-and-rename storms into a single reload.

`notify` spawns its own internal watcher thread, so the module
indirectly owns two non-audio threads (watcher + loader). Acceptable;
both are scoped to the module instance and joined on `Drop`.

### `auto_scan` global config

Live FS observation is right for `patch_player` and CLAP. It is
wrong for golden tests (non-determinism) and for sealed bundles
distributed without writable sample directories. We gate the
watcher behind a host-level `auto_scan` flag:

- `auto_scan = true`: spawn loader+watcher thread, double-buffer
  swap path is live.
- `auto_scan = false`: synchronous load at build, single buffer,
  no watcher, no swap. Cheapest mode.

The two paths are **separate storage variants** in the module —
`Storage::Live { front, swap, _watcher }` and `Storage::Sealed
{ samples }` — to keep the audio-path branch out of the hot loop.

The flag rides on the global host config introduced by ADR 0075 /
E148. Until that ticket lands, the v1 implementation reads
`auto_scan` from an environment variable
(`PATCHES_SAMPLER_AUTO_SCAN`) defaulting to true, and a follow-up
ticket migrates the read to `GlobalConfig::auto_scan_samples` once
the schema field exists.

### Crate placement

The module lives in `patches-bundles/patches-drums/src/sampler.rs`
and is registered by `patches_drums::register`. The `notify` crate
is added to `patches-drums`'s `Cargo.toml`. WAV decode reuses
`patches_io::read_mono`.

## Consequences

### Positive

- Async load matches DAW-resident conventions: the structural
  rebuild returns instantly, audio starts silent, and slots fill
  in as decoding completes.
- Live FS observation gives a frictionless drop-files-and-hear-them
  workflow for live-coding.
- Indexed-range arena + double-buffer is a single, contained
  pattern: lock-free hot path, no audio-thread allocation, no
  audio-thread drops, no shared ownership.
- Two storage variants keep the sealed path cheap (no watcher, no
  swap check) and the live path obvious. No runtime branch on a
  per-tick "is live?" flag.
- ~1 ms fade on swap removes audible clicks without retention
  complexity (no `Arc`, no generation counters, no voice migration).

### Negative

- One new dependency (`notify`) in `patches-drums`. Approve before
  adding.
- Two non-audio threads per `Sampler` instance when `auto_scan` is
  on. Acceptable: instances are few (one per drum kit per patch),
  and they join cleanly on `Drop`.
- Mid-playback swaps that change a slot's audio are smoothed by
  the fade, but the audio still changes — there is no perfect
  retention. The user contract is "live edits cause near-silent
  reloads, not zero-impact reloads."
- The MVP env-var-driven `auto_scan` is a small migration cost
  once E148's `GlobalConfig` schema lands.

### Out of scope

- Velocity layers, pitch-mapping zones, round-robin: deferred.
  Drum-rack equivalence is a non-goal (the user's brief).
- Mipmap / bandlimited octave layers: dropped — drums rarely
  tuned far up.
- Pitch/time-shifting beyond simple phase-increment playback: no
  elastique in the stack.
- Stereo file support: v1 sums to mono on load. A `StereoSampler`
  or per-slot stereo flag can come later.
- CV-variable `start`/`end`/`loop_*`: parameters only in v1; can be
  promoted to CV-or-param inputs later without breaking patches.
- Per-patch sidecar persistence of edited slot ranges: out of
  scope; `loop_start[i]` etc. are DSL parameters, edited by
  re-saving the patch.
