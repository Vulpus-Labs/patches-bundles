# patches-vintage

Modules evoking 1970s–80s analog-synth and analog-delay character:
Juno-style DCOs, ladder and OTA-C VCFs, bucket-brigade delay lines,
log/exp companding, and effects built on top of them (chorus, flanger,
multi-tap delay, reverb). No named-hardware modelling — the building
blocks are plausible generic analogues of the parts a pedal-, rack- or
synth-builder of the era would have reached for.

For the BBD signal-path derivation and real-time-safety details, see
[TECHNICAL.md](TECHNICAL.md).

## Architecture

The crate is layered:

1. **Primitives** — reusable DSP cores with no module-protocol surface:
   - `bbd_clock` — BBD half-clock generator (write/read tick stream with
     sub-sample timing).
   - `bbd_filter_proto` — continuous-time complex-pole filter bank with
     closed-form sub-sample evaluation (`evaluate(τ, u)` and
     `advance_by(Δτ, u)`).
   - `bbd_proto` — composition of the above with a bucket ring and
     soft-saturation; the actual sample-rate-decoupled BBD engine.
   - `bbd` — ergonomic wrapper (`Bbd`, `BbdDevice` presets for 256 /
     1024 / 4096 stages) that picks default anti-imaging / reconstruction
     filter shapes and a sample-rate-appropriate smoothing cadence.
   - `compander` — 2:1 log encode / 1:2 exp decode pair modelled on a
     generic integrated compander (full-wave rectifier → asymmetric-
     attack/release one-pole averager → variable gain cell). Not used
     by the chorus (the period's chorus pedals did not compand); used
     by the delay and flangers where the pedal tradition does compand.

2. **Modules** — wire primitives into the Patches `Module` trait:

| Module             | Role                                                                                                                                                  |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `VDco`             | Juno-style DCO: one phasor drives saw, variable-width pulse, ÷2 sub-square and a wavefolded triangle, with internal noise mix and PolyBLEP hard-sync. |
| `VPolyDco`         | 16-voice polyphonic sibling of `VDco` sharing the same voice core.                                                                                    |
| `VLadder`          | Mono 4-pole ZDF transistor-ladder LPF (`sharp` / `smooth` voicings) wrapping the `patches-dsp` ladder kernel.                                         |
| `VPolyLadder`      | 16-voice polyphonic sibling of `VLadder`.                                                                                                             |
| `VOtaVcf`          | Mono R3109/IR3109-style 4-pole OTA-C LPF with per-stage `tanh` saturation.                                                                            |
| `VOtaPolyVcf`      | 16-voice polyphonic sibling of `VOtaVcf` with `GLOBAL_DRIFT` backplane support.                                                                       |
| `VChorus`          | Single-BBD chorus, two voicings (bright / dark) with preset LFO modes.                                                                                |
| `VFlanger`         | Mono flanger: LF bypass split, companded BBD on the HF band, feedback-driven comb.                                                                    |
| `VFlangerStereo`   | Two independent BBD chains sharing one triangle LFO, right channel on the inverted phase.                                                             |
| `VBbd`             | N-tap BBD delay: per-tap compressor → 4096-stage BBD → expander, per-tap gain and self-feedback, global dry/wet.                                      |
| `VStereoBbd`       | Two parallel `VBbd`-style chains (L, R) with per-tap delay/gain/feedback.                                                                             |
| `VReverb`          | 8-line Hadamard FDN reverb built entirely out of 1024-stage BBD lines; no compander (see below).                                                      |

## How the pieces fit

- **The BBD is the voice.** All effects route their wet path through a
  bucket-brigade line. The HF rolloff, gentle bucket saturation, and
  (at long delays) clock-aliasing image-fold are what give each module
  its character. None of the modules expose a "tone" or "drive" knob —
  the colour comes from the BBD itself, not a post-EQ.

- **Compander bracketing.** The delay and flangers wrap each BBD in a
  compressor/expander pair. This is the standard period topology for
  single-pass BBD effects: the compressor lifts quiet program material
  above the bucket noise floor, and the expander restores dynamics on
  the way out. The program-dependent residual is part of the sound.

- **No compander on the reverb.** The compander's round-trip gain is
  only unity at its reference level; away from reference it goes as
  `(ref / level)^0.25`. Harmless on a single-pass delay, but inside a
  feedback-delay network the recursive loop drives it into an unstable
  cycle (quiet tail → loop gain > 1 → runaway → saturation → compressor
  drags it silent → repeat). `VReverb` relies on the BBDs' own anti-
  imaging filters and bucket saturation for both colour and damping.

- **Delay smoothing.** LFO-driven modulation updates BBD delay on a
  power-of-two stride (the `Bbd` exposes `smoothing_interval()` in
  samples). Between updates the BBD linearly interpolates both the
  clock period and the per-pole state-transition factors, so no `exp()`
  is called on the audio-rate hot path during modulation.

## Registering

`patches-vintage` is **not** part of the default in-process registry.
It ships as a stdlib FFI bundle (cdylib) loaded at runtime by
`PluginScanner` — see `patches_ffi::scanner::stdlib_scanner` and the
`PATCHES_PLUGIN_PATH` search-path policy (ADR 0073, ticket 0876). Hosts
such as `patches-player` and `patches-clap` run the stdlib scan on
startup; pass `--no-stdlib` to skip it.

`patches_vintage::register(&mut registry)` is retained as an
in-process shim for tests and bespoke embedders, but no first-party
host calls it.

## Real-time safety

- All buffers allocated in each module's `new`. `process` does no
  allocation, no locking, and no syscalls.
- BBD internals use a SoA pole layout and an incremental complex
  phasor so that `exp()` fires at most once per pole per host sample on
  the steady path.
- Parameter CV and parameter-map updates come in on the Periodic tick;
  audio-rate CV goes through the same smoothing stride as the LFO.
