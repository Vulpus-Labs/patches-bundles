# ADR 0002 — Bridged-T resonator family

## Status

Proposed (2026-05-18). Amended 2026-05-18 — substrate switched to TPT
SVF and pitch-droop mechanism made explicit (amplitude → frequency FM
per Mutable Instruments Plaits's `AnalogBassDrum`) rather than emergent
from a feedback saturator. See §"Linear core" and §"Nonlinearity and
pitch droop" below.

Amended 2026-05-19 — self-FM (the half-wave-rectified `lp` →
cutoff-offset mechanism) did **not** deliver audible pitch droop in
practice. Attack-FM does work and is the trigger-locked pitch-lift
mechanism going forward. The `drive` parameter is retained on the API
but now controls primarily the output saturator amount, not pitch
motion. See §"Nonlinearity and pitch droop — 2026-05-19 update"
below. Future struck-resonator voices (e.g. a forthcoming `Snare2`)
should not claim droop unless attack-FM gets them there.

## Context

The drums bundle today follows one architectural pattern for the
pitched membrane voices (`Kick`, `Tom`, and to a lesser extent
`Claves`): a phase-accumulator oscillator driven by a separate pitch
envelope, a separate amplitude envelope, and — for `Kick` — a separate
click layer. Three independent envelopes, decoupled pitch and
amplitude, oscillator-as-source.

This is the classic "driven oscillator" school of analog kick design,
and it produces a recognisable percussive shape: punchy attack, long
sustained body, harmonically rich (triangle-through-saturation type
spectra), pitch and decay independently controllable.

It is **not**, however, the only school. A categorically different
analog approach builds the pitched voice around a high-Q resonator
(typically a bridged-T network in the feedback path of an op-amp) that
is struck with a short trigger pulse. The resonator rings at its
natural frequency, decays at a rate set by its Q, and acquires
character from the nonlinearity in its own feedback path. There is no
separate pitch envelope; pitch droop emerges from the soft-clipping in
the loop. There is no separate amplitude envelope; decay is the Q.
There is no separate click layer; the excitation pulse shape is the
attack.

The two architectures are sonically distinct:

- **Driven oscillator** (existing `Kick`, `Tom`): tonal, sine-dominant
  or harmonically rich depending on shaper, character lives in the
  envelopes and the saturator, parameter surface is large
  (pitch / sweep / sweep_time / decay / drive / click).
- **Struck resonator** (this ADR): emergent pitch droop coupled to
  amplitude, decay coupled to Q, character lives in the filter
  nonlinearity and the excitation shape, parameter surface is small
  (tune / Q / pulse-shape / clip).

Adding the second school to the bundle is genuinely a different
paradigm rather than a different preset of the first. The same family
also unlocks `Claves` cascading (two resonators with the first's
ringing burst triggering the second, giving effective Q far past what
a single stage can sustain) and a more idiomatic tom voice (the
amplitude-driven pitch envelope familiar from the early analog drum
machines is a feature of the bridge nonlinearity, not a separate
modulation source).

## Decision

### Vocabulary

- **`Kick2`, `Tom2`, `Claves2`** — three new modules implementing the
  struck-resonator architecture. Suffix `2` denotes "second design
  approach", not "version 2". The existing `Kick`, `Tom`, `Claves`
  remain unchanged; the two families coexist.
- **`BridgedT`** — new primitive in `patches-drums/src/primitives/`
  wrapping the existing Chamberlin SVF kernel with a soft-clipped
  feedback path. Bandpass-tap output. Shared by all three new modules.
- **`Excitation`** — new primitive producing a short shaped pulse
  (1–5 ms typical). Several pulse shapes selectable. Shared by all
  three new modules.

### Technical underpinnings

#### Linear core

A bridged-T network in the feedback path of an op-amp has, to first
order, a continuous-time transfer function of the form

```
H(s) = (s · ω₀ / Q) / (s² + s · ω₀ / Q + ω₀²)
```

with `ω₀ = 1 / √(R · C)` and `Q` set by component ratios in the
bridge. This is a high-Q bandpass biquad. Two discretisations are
plausible: a bilinear biquad, or a state-variable filter. We use a
**topology-preserving (TPT / zero-delay-feedback) SVF** per Vadim
Zavalishin's "The Art of VA Filter Design", Ch. 5.

TPT — not Chamberlin — because this voice modulates the cutoff `f`
**per sample** (see §"Nonlinearity and pitch droop"). Chamberlin's
forward-Euler discretisation pre-warps with `f = 2·sin(π·fc/sr)`,
which is correct only when `fc` is static; audio-rate `fc` modulation
introduces coefficient errors that compound through the integrator
pair and detune the resonance peak. TPT uses trapezoidal integrators
with implicit zero-delay feedback solved analytically; its pre-warp
(`g = tan(π·fc/sr)`) is cancelled by the bilinear transform under any
`fc` trajectory, so per-sample retuning stays mathematically clean.
TPT is also unconditionally stable in normalised form, which removes
the explicit `stability_clamp(f, d)` dance Chamberlin requires at
high Q (`patches_dsp::SvfKernel::stability_clamp` is the existing
example).

The TPT SVF is vendored locally as `patches-drums/src/primitives/
tpt_svf.rs`. Patches-dsp inclusion is gated on a second consumer
appearing; until then, keeping it crate-local avoids prematurely
pinning an API on a primitive whose shape we are still validating.

#### Nonlinearity and pitch droop

The interesting sonic features of an analog struck resonator do not
come from the linear response. They come from amplitude-coupled
behaviour: at attack the voice has a pitch lift that decays back to
`ω₀`, the body acquires harmonic colour from soft-clipping, and the
attack itself adds a brief frequency excursion above `ω₀` from a
short FM pulse.

The earlier draft of this ADR claimed the pitch droop would emerge
from a feedback saturator on the SVF's `bp_state`. Empirical testing
shows this is wrong: applying `saturate(bp_state, clip)` to the
`q · bp` damping term of a Chamberlin SVF changes the effective Q
(and adds odd harmonics), but does not shift the complex-pole angle
— so pitch stays at `ω₀` regardless of amplitude. Linear analysis of
the discrete recurrence confirms this. The analog circuit's droop
comes from how the op-amp's nonlinear loop behaviour shifts the
bridged-T network's pole locations; the digital SVF does not carry
that mechanism.

We therefore model droop **explicitly**, following Émilie Gillet's
[Mutable Instruments Plaits `AnalogBassDrum`](https://github.com/pichenettes/eurorack/blob/master/plaits/dsp/drums/analog_bass_drum.h):

1. **Self-FM** — half-wave-rectify the SVF's `lp` tap from the
   previous sample, scale by a `drive` parameter, and add to the
   instantaneous cutoff before the next tick. As amplitude decays,
   the FM offset shrinks and `f` returns to the base tune. Direction
   is upward at attack, settling to `ω₀` — the 808 "thump → boom".
2. **Attack-FM** — a short rectangular pulse (≈ 6 ms) lowpassed at
   ≈ 10 kHz fires on trigger, scaled by an `attack` parameter, and
   adds an additional brief lift to `f`. Distinct from self-FM in
   that it is trigger-locked, not amplitude-locked, and gives the
   percussive "click → swoop" character.
3. **Saturator** — kept on the `bp` output as a soft-clipper that
   adds odd-harmonic content. Its placement is post-output rather
   than in the feedback path: the TPT SVF's implicit equation
   resolution does not tolerate a nonlinearity inside the integrator
   loop without a Newton-style solve, and post-output saturation
   gives the harmonic dirt we want without that complication.

The `drive` and `attack` parameters expose these two FM paths. Both
are normalised in `[0, 1]`. The saturator amount is folded into
`drive` with a fixed internal ratio so the parameter surface stays
small.

For `Tom2` this is the source of the amplitude-driven glide familiar
from the analog tom voices — same self-FM mechanism, larger default
`drive`. `Claves2` does not use FM; its character comes from the
cascade described below.

#### Nonlinearity and pitch droop — 2026-05-19 update

Implementation against the design above landed and was tested
empirically. **Attack-FM works as intended**; the brief trigger-locked
pulse on `f` produces an audible lift on strike that settles back to
`ω₀`, giving Kick2 its "thump → tone" character. `pitch_droop_with_attack_fm`
in [kick2.rs:331](../src/kick2.rs#L331) verifies this and matches what
the ear hears.

**Self-FM does not.** The half-wave-rectified `lp` → cutoff-offset
mechanism produces a measurable but inaudible pitch shift. The
existing `pitch_droop_with_self_fm` test in
[kick2.rs:346](../src/kick2.rs#L346) passes only because its
acceptance threshold (`early > late + 10 Hz` over a 1024-sample early
window where one FFT bin is ≈ 43 Hz at 44.1 kHz) clears at a
sub-perceptible offset. By ear, `drive` does not move pitch; cranking
it past 1.0 to compensate hits the cutoff `CONSTRAIN` clamp and
produces zipper / saturation artefacts before the FM offset becomes
audible.

Root cause: rectified `lp` magnitude at non-distorting drive amounts
is too small a fraction of `ω₀` to perceptibly shift the resonant
peak. Plaits's `AnalogBassDrum` survives this in context — its
excitation, output stage, and overall voice shape carry the impression
of droop that the FM contribution alone does not. Lifted into
isolation as a generic primitive mechanism, it does not stand up.

Decision going forward:

- **`drive` is retained on `Kick2` / `Tom2` and on `BridgedT`'s
  `fm_offset` argument**, but the API contract is reframed: `drive`
  primarily scales the output saturator, with a small residual
  self-FM contribution that nudges the spectral content without
  claiming an audible pitch envelope.
- **Attack-FM is the canonical pitch-motion mechanism** for this
  family. If a struck-resonator voice needs trigger-locked pitch
  motion, route it through the attack-FM path.
- **No droop in `Snare2` and the metal voices.** Real snare bodies
  and metal voices do not noticeably droop; the previous draft's
  framing of self-FM as a universal droop primitive does not survive
  contact with the implementation. `Snare2` ships with `attack` only;
  any pitch motion comes from the trigger-locked attack pulse.
- **`Tom2`'s "amplitude-driven glide" framing is downgraded** to "a
  brief attack-FM lift". The drafted analog-tom-style amplitude →
  pitch envelope is not happening with this substrate. Acceptable —
  the tom still sounds like a tom; the framing in the rustdoc and
  the §"Nonlinearity and pitch droop" section above is now aspirational
  rather than descriptive.

The 2026-05-18 amendment's design framing stays in place as historical
context; this section overrides its claims about self-FM. If a later
substrate change (Newton-solved in-loop nonlinearity, op-amp-saturation
WDF, etc.) revives a working droop mechanism, this section gets
re-amended with that result.

#### Excitation shape

A single-sample Dirac impulse into a high-Q biquad will ring at the
right frequency but sound thin. The analog circuits do not deliver a
Dirac; they deliver a short asymmetric pulse, a couple of milliseconds
long, with energy distributed in time and spectrum. The pulse shape
matters more than its exact spectral content — different shapes give
weight, thump, click, or snap from the same resonator.

`Excitation` exposes a small set of shapes:

- `Dirac` — single-sample unit impulse. Reference.
- `ExpDecay { tau_ms }` — `exp(-t/τ)` envelope, default τ ≈ 1 ms.
- `HalfSine { ms }` — half-cycle sine, default 2 ms.
- `FilteredClick { lp_hz, ms }` — white-noise burst through a
  one-pole low-pass.

The shape is a structural-resolution choice for the module (set at
build time) plus a `pulse_ms` realtime parameter that scales the
shape's duration. This keeps the audio-thread path branch-free per
sample once the shape is selected.

#### Cascading (Claves2)

Two `BridgedT` stages in series, but with the trigger relationship
inverted from the obvious "stage 1 → stage 2 audio in" form:

- Stage 1 is excited by the trigger pulse and rings at its tuned
  frequency.
- Stage 1's bandpass output, passed through a level threshold and a
  rising-edge detector, becomes the **excitation pulse** for stage 2.
- Stage 2 rings at the same nominal tune (or a small offset) and is
  re-excited by every cycle of stage 1's burst.

The result is that stage 2's effective Q is far higher than its
literal Q parameter — every period of stage 1's burst kicks it again
before its own decay matters. The audible effect is a short, very
bright, fast-decaying click with extended ring relative to a
single-stage configuration. Typical total length ≈ 25 ms.

### Parameter surfaces

| Module | Params | Notes |
|--------|--------|-------|
| `Kick2` | `tune` (Hz), `q`, `pulse_shape` (struct), `pulse_ms`, `drive`, `attack` | No separate pitch envelope, no separate amp envelope, no click layer. Decay is `q`; pitch droop is `drive` (self-FM + saturator); strike lift is `attack` (attack-FM). |
| `Tom2` | `tune` (Hz), `q`, `pulse_shape` (struct), `pulse_ms`, `drive`, `attack` | Same surface as `Kick2`, different defaults (higher tune, lower Q, stronger default `drive`). The amplitude-coupled glide is `drive`. |
| `Claves2` | `tune` (Hz), `q`, `cascade_mix` (0 → single-stage; 1 → fully cascaded), `pulse_ms`, `clip` | `cascade_mix` mixes stage-1-only vs stage-1+stage-2; lets the module collapse to single-stage at one end of the knob. No FM — the cascade is the character. `clip` here is just the output saturator. |

`pulse_shape` is structural rather than realtime to keep the
audio-path branch-free; switching shape requires a `restructure`. The
common per-shape parameter (duration) lives in `pulse_ms` and is
realtime.

### Module registration and FFI

The three new modules are added to `patches_drums::register` and to
the `export_modules!` invocation in [lib.rs](../src/lib.rs#L41-L66)
with ABI version 1.

### Numerical stability

The TPT SVF is unconditionally stable in normalised form, so the
explicit `stability_clamp` Chamberlin needs goes away. At Q values
typical of struck-resonator voices (50+) and low frequencies (sub-
100 Hz for `Kick2`), the integrator state can still flirt with
denormals on long ringdowns; `BridgedT::tick` sanitises state writes
to `0.0` on non-finite, mirroring the pattern used in
`patches_dsp::SvfKernel`. Per-sample `set_f` (driven by self-FM and
attack-FM) recomputes the TPT coefficients each tick; cost is one
`tan` per sample, which is acceptable for three drum voices at the
sample rates this bundle targets.

If the `drive * self_fm + attack * attack_fm` total ever drives
`g = tan(π·f/sr)` toward Nyquist, the modulation is clamped at the
voice level so `f` stays in `[tune_base, 0.4 · sr]` — matching Plaits's
`CONSTRAIN(f, 0.0f, 0.4f)`.

## Consequences

### Positive

- The bundle gains a genuinely distinct second paradigm for pitched
  voices rather than a re-preset of the existing one.
- One small primitive (`BridgedT`) + one small primitive
  (`Excitation`) backs three new modules. Marginal cost of `Tom2` and
  `Claves2` after `Kick2` lands is small.
- The cascaded `Claves2` topology is otherwise unavailable in the
  bundle and is a known-good design for extended-Q clicks.
- Existing `Kick`, `Tom`, `Claves` keep their behaviour. Users who
  prefer the driven-oscillator sound lose nothing.
- The new primitives are usable by anything in the bundle later — a
  bridged-T conga, a cowbell with a bridged-T body, or a
  bridged-T-flavoured snare are then incremental.

### Negative

- Parameter-surface mismatch between sibling modules: `Kick` exposes
  six knobs, `Kick2` exposes four-or-five. Hosts presenting both
  alongside each other will look slightly inconsistent. Acceptable —
  the asymmetry is the point.
- Two new module names in the bundle's namespace. `Kick2` and friends
  are short and obvious but do require docs.
- Soft-clipped SVF feedback is a small nonlinear feedback loop; at
  extreme Q × clip combinations it can self-oscillate or produce
  pitched limit cycles. We constrain the parameter ranges to keep this
  out of normal operation and document the failure mode.

### Out of scope

- A bridged-T conga or cowbell. Worth doing later but not in this
  epic.
- A modal / banked-resonator kick (`Kick3`?). Different paradigm
  again, deserves its own ADR.
- An FM/PD kick. Cross-bundle dependency on Prism. Separate epic.
- Replacing the existing `Kick`, `Tom`, `Claves` implementations. The
  two families coexist; users pick.
- Per-voice excitation routing (sending an external signal as
  excitation rather than a synthesised pulse). Possible later
  extension, not v1.
