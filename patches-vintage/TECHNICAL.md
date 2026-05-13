# patches-vintage — BBD technical notes

This document describes how the bucket-brigade delay (BBD) core in
`patches-vintage` is constructed. The design follows the combined
BBD + input/output filter model of

> Martin Holters and Julian D. Parker, **"A Combined Model for a
> Bucket Brigade Device and its Input and Output Filters"**,
> Proc. of the 21st International Conference on Digital Audio Effects
> (DAFx-18), Aveiro, Portugal, September 2018.

Holters & Parker (hereafter **H-P**) observe that a BBD chip cannot be
treated as a fractional-sample delay at host rate: the BBD is clocked
asynchronously to the host, its clock rate sweeps with delay time, and
the analog anti-imaging filter in front of it and reconstruction filter
behind it are what shape the chip's characteristic voice — including
the image-folding you hear at long delay times when the bucket clock
drops below twice the audio bandwidth. H-P give a closed-form recipe:
evaluate the continuous-time input filter at the exact sub-sample
instants at which bucket writes occur, and evolve the continuous-time
output filter through held-bucket segments between read ticks. The
implementation here is a clean-room realisation of that recipe; it is
not a port of the H-P source code.

The code is split into three layers so the numerics can be tested in
isolation. Everything is real-time-safe: all buffers are allocated at
construction, and the hot path performs no allocations, no locks, and
at most one `exp()` call per pole per host sample on the steady path.

## Layer 1 — BBD clock (`bbd_clock.rs`)

A BBD chip clocks at

    f_clock = 2 · stages / delay_seconds

alternating **write** half-phases (input sampled into a bucket) with
**read** half-phases (bucket sampled out). Each full bucket cycle is
`bbd_ts = 1 / f_clock` seconds. The clock is asynchronous to the host
sample rate:

- at short delays, many half-ticks fire per host sample;
- at long delays, one tick may span several host samples.

`BbdClock::step(on_tick)` advances one host sample and emits zero or
more `Tick { phase, tau }` values, where `τ ∈ [0, 1)` is the sub-sample
fraction at which the tick fires. State is a single sub-sample carry
`tn ∈ [0, host_ts)` plus a write/read toggle; no allocations. A floor
of `host_ts · 0.01` on `bbd_ts` prevents the inner loop from firing an
unbounded number of ticks at a degenerate clock rate.

Tests verify tick density against `host_ts / bbd_ts` at short and long
delays, monotonic tick indices, phase alternation, and that τ stays in
`[0, 1)` under every clock rate.

## Layer 2 — continuous-time pole bank (`bbd_filter_proto.rs`)

A single complex one-pole section is the ODE

    dx/dt = p · x + u(t)

with `u(t)` held piecewise-constant over one host sample. The exact
solution over a sub-sample interval of length `τ · Ts`, with `Ts` the
host sample period, is

    φ(τ) = exp(p · τ · Ts)
    ψ(τ) = (φ(τ) − 1) / p
    y(n·Ts + τ·Ts) = φ(τ) · x[n] + ψ(τ) · u[n]

Two operations matter on the audio path:

- **`evaluate(τ, u)`** — compute `y` at an arbitrary sub-sample time
  without modifying state. This is how input-filter output is sampled
  at each Write tick's `τ`.
- **`advance_by(Δτ, u)`** — evolve state by a sub-sample interval
  `Δτ · Ts` with `u` held. This is how the output filter is walked
  through held-bucket segments delimited by Read ticks.

`advance(u)` is the special case `advance_by(1, u)`; because both use
the same closed form, the stitch between host samples is analytical,
not approximate.

Real poles must come as conjugate pairs for a real-valued output. The
bank stores one pole per pair and doubles the real part of the residue
sum internally. A **SoA** layout (`ConjPairPoleBankSoa`) keeps pole,
residue, state, and precomputed `φ(1)` / `ψ(1)` in parallel arrays so
each pole update is a short, branch-free straight-line sequence.

### Incremental phasor

Within a single host sample, successive Write ticks are spaced by the
same sub-sample step `Δτ = 2 · bbd_ts / host_ts` (a full bucket cycle
between successive Writes). So the per-pole phasor

    φ(τ_k) = exp(p · τ_k · Ts)

can be built incrementally:

    φ(τ_0)     = exp(p · τ_0 · Ts)    ← one `exp()` at the first Write
    φ(τ_{k+1}) = φ(τ_k) · α,           α = exp(p · Δτ · Ts)

with `α` precomputed whenever the delay changes. Every Write tick
after the first is a complex multiply. The same trick applies on the
output path for the middle held-bucket segments (all of which share
the same `Δτ`); the first and last segments have variable length and
use the inline `exp`.

## Layer 3 — composition (`bbd_proto.rs`, `bbd/mod.rs`)

The full BBD is a clock + input bank + bucket ring + output bank.

Per host sample:

1. Advance the delay-smoothing ramp (see below).
2. Drive the clock; for every tick inside the sample:
   - **Write:** evaluate the input bank at `τ` to get the bucket charge,
     apply optional soft-saturation `tanh(drive · x) / drive`, write
     into the ring at the current pointer, advance the pointer.
   - **Read:** record `(τ, bucket_value)` into a pre-allocated scratch
     vector; the ring pointer does *not* move — reads lag writes by
     `stages` bucket-cycles.
3. After the tick loop: `advance(u)` the input bank once with the
   current host input (rolls its state by one host sample).
4. Walk the output bank through the recorded segments. With Read ticks
   at sub-sample times `τ_1, τ_2, …` and a retained `last_bucket_read`
   from the previous sample:
     - segment `[0, τ_1)` at value `last_bucket_read`;
     - segment `[τ_i, τ_{i+1})` at value `bucket[i]`;
     - tail `[τ_last, 1)` at value `bucket[last]`.
   Each segment is one `advance_by(Δτ, u)` on every pole.
5. Output = `Re(Σ r_k · x_k)` at the end of the host sample.

Treating the bucket sequence as piecewise-constant (rather than a train
of impulsive deltas) is essential: a delta formulation loses DC,
because a stable bucket value has zero delta. The piecewise-constant
view captures both the DC response and the transient ring at bucket-
value discontinuities, which is what H-P's derivation requires.

### Aliasing and image-folding

No anti-aliasing is bolted on. The fold emerges from the geometry: at
long delays the BBD clock drops below `2 · audio_band`, and any input
energy above `f_clock / 2` is resampled onto the sparse Write-tick
grid and returned as an aliased component. A unit test drives 15 kHz
into an 80 ms / 1024-stage BBD (`f_clock ≈ 25.6 kHz`) and asserts
that the expected fold at `|15 000 − 25 600| = 10 600 Hz` has
significant energy at the output — the behaviour a naive host-rate
lowpass-cascade BBD would miss.

### Filter shapes

`bbd::default_pole_pairs()` returns two well-damped conjugate pairs
(Q ≈ 0.3) giving a non-peaking ~4-pole lowpass rolling off from
~6 kHz, used for both the input anti-imaging filter and the output
reconstruction filter. The damping is deliberate: the combined
input × output transfer is required to stay below unity across the
entire passband so that feedback networks (the FDN reverb, or a delay
with self-feedback) cannot gain at any in-band frequency.

Residues are normalised so the DC gain `2 · Σ Re(−r / p)` is exactly
unity. This is what makes the DC-gain cross-check between the H-P-style
prototype and the reference implementation robust to topology choices.

### Delay-modulation smoothing

LFO-driven delay modulation calls `set_delay` on a fixed stride (a
power of two close to `sample_rate / 3000`, so ~333 µs; the caller
gates with `counter & (interval − 1) == 0`). Each call:

- targets a new `bbd_ts = delay / (2 · stages)`;
- the first call snaps; subsequent calls schedule a linear ramp of
  `bbd_ts` and of each pole's per-tick `α` across `smoothing_interval`
  samples.

Between updates, the filter interpolates `α` toward target linearly
and resynthesises `φ` per host sample by one complex multiply per
pole. The `exp()` that would otherwise fire on every delay change is
paid at most once per stride, not once per sample — critical for
audio-rate modulation.

### Bucket saturation

`BbdDevice::saturation_drive > 0` applies `tanh(drive · charge) / drive`
to the input bank's output before it is written to the bucket. This is
unity-gain at zero, soft-clips with increasing magnitude, and gives the
characteristic gentle overload when drive is pushed. The presets
default to `1.2`.

## Reference implementation and tests

`bbd_proto.rs` contains the full H-P-style realisation. `bbd/mod.rs`
wraps it with preset filters and device sizes (`BBD_256`, `BBD_1024`,
`BBD_4096`). Cross-tests assert invariants that should be topology-
free between prototype and wrapper:

- silence in → silence out;
- impulse peak at the commanded delay (± 2 ms of group-delay
  tolerance);
- DC gain within ± 1 dB of unity, and within ± 0.5 dB of the
  reference;
- a sustained passband sine shows no sub-Hz amplitude drift (the
  invariant that broke an older simple-cascade BBD port);
- image-fold energy at the predicted alias frequency for long delays.

## Further reading

- Holters & Parker, *A Combined Model for a Bucket Brigade Device and
  its Input and Output Filters*, DAFx-18, 2018 — the derivation this
  implementation follows.
- `bbd_clock.rs`, `bbd_filter_proto.rs`, `bbd_proto.rs`, `bbd/mod.rs`
  — each carries its own module-level doc comment with implementation
  detail beyond what is summarised here.
