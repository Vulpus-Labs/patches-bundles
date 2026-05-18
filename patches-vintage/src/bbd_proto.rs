//! Composition prototype: [`BbdClock`] + [`ContinuousPoleBank`] +
//! bucket ring, with full sub-sample evaluation on both the input and
//! output paths.
//!
//! **Input path**: at each Write tick the input filter bank is
//! evaluated at the tick's exact sub-sample `τ` to produce the bucket
//! charge. Input state advances once per host sample.
//!
//! **Output path**: the bucket sequence is treated as a piecewise-
//! constant signal whose segment boundaries fall at Read ticks. For
//! each segment `[τ_start, τ_end)` within a host sample with held
//! bucket value `B`, the output bank's state evolves via the closed-
//! form integration `x_new = φ(Δτ)·x + ψ(Δτ)·B` per pole. This
//! correctly captures both the steady-state (DC) response and the
//! transient ring at bucket-value discontinuities — a pure impulsive-
//! delta formulation loses DC because steady buckets have zero delta.
//! Output at end-of-sample = `Re(Σ r_k · x_k)`.
//!
//! Aliasing emerges naturally: at long delays the BBD clock drops
//! below 2× the audio band, so input energy above `clock/2` folds
//! back through the sub-sample sampling pattern. Tested explicitly.

use crate::bbd_clock::{BbdClock, TickPhase};
use crate::bbd_filter_proto::{Complex32, ConjPairPoleBankSoa, ContinuousPoleBank};
use patches_sdk::BoundedRandomWalk;

/// Max multiplicative swing applied to `bbd_ts` at `jitter_amount = 1.0`.
/// Matches ≈ ±17 cents of pitch wobble on the delayed signal.
const JITTER_MAX_DEPTH: f32 = 0.03;

/// Samples between random-walk advances for BBD clock jitter. ~1.3 ms at
/// 48 kHz, giving a characteristic wander in the low-Hz range.
const JITTER_WALK_INTERVAL: u32 = 64;

/// Walk step size tuned so the walk traverses a meaningful fraction of
/// its [-1, 1] range at audio-musical rates.
const JITTER_WALK_STEP: f32 = 0.03;
use patches_dsp::approximate::fast_tanh;

/// Placeholder smoothing interval for the legacy AoS constructor,
/// which does not use the SoA incremental-phasor / ramp machinery.
/// Real SoA clients receive their interval at construction time.
const AOS_PLACEHOLDER_INTERVAL: u32 = 1;

/// Internal bank storage: full AoS bank for the general constructor,
/// SoA conjugate-pair bank for the optimised path. The size delta is
/// intentional — a given `BbdProto` lives with one variant for its
/// lifetime and we avoid a Box on the audio path.
#[allow(clippy::large_enum_variant)]
enum Banks {
    Aos {
        input: ContinuousPoleBank,
        output: ContinuousPoleBank,
    },
    Soa {
        input: ConjPairPoleBankSoa,
        output: ConjPairPoleBankSoa,
        /// Per-pole phasor scratch reused across ticks within a
        /// sample. Running `phi` for the input path's Write ticks.
        phi_re: Vec<f32>,
        phi_im: Vec<f32>,
    },
}

/// End-to-end BBD driven by an explicit clock and sub-sample-evaluated
/// continuous-time filter banks.
pub struct BbdProto {
    clock: BbdClock,
    banks: Banks,

    buckets: Vec<f32>,
    /// Shared bucket pointer. Advances on Write ticks only; Read ticks
    /// sample at the current position, so the read always lags the
    /// write by the ring length (≈ `stages`) worth of Write intervals.
    buffer_ptr: usize,

    /// Most recent Read tick's bucket value, held across ticks and
    /// host samples as the current segment value feeding the output
    /// bank until the next Read tick fires.
    last_bucket_read: f32,

    /// (τ, bucket_value) pairs recorded during the tick loop — each is
    /// a segment boundary where the held input to the output bank
    /// changes. Pre-allocated, cleared per sample to avoid alloc.
    read_events: Vec<(f32, f32)>,

    /// Bucket-write saturation drive. `0.0` disables. Applied as
    /// `tanh(drive · charge) / drive` — unity gain at zero, soft-clips
    /// as magnitude grows.
    saturation_drive: f32,
    saturation_inv_drive: f32,

    stages: usize,

    /// Smoothing interval in samples for delay-modulation ramps on
    /// the SoA path. Clients are expected to call [`Self::set_delay`]
    /// every `smoothing_interval` samples; between calls the filter
    /// linearly interpolates `bbd_ts` and the per-pole phasor `alpha`.
    smoothing_interval: u32,
    inv_smoothing_interval: f32,

    /// Current `bbd_ts` (in seconds) driving the clock. On the SoA
    /// path it is ramped toward `bbd_ts_target` one sample at a time
    /// across `smoothing_interval` samples.
    bbd_ts_cur: f32,
    bbd_ts_target: f32,
    bbd_ts_step: f32,
    /// Samples remaining in the current ramp. Zero when no ramp in
    /// flight.
    ramp_samples_remaining: u32,
    /// Set to true after the first `set_delay` so subsequent calls
    /// know to schedule a ramp rather than snap.
    has_delay_set: bool,

    /// Clock-jitter amount in `[0, 1]`. Scales an internal random walk
    /// into a multiplicative perturbation of `bbd_ts` fed to the clock.
    /// `0.0` disables the jitter path entirely — the walk is not
    /// advanced and the clock sees the unperturbed `bbd_ts_cur`.
    jitter_amount: f32,
    jitter_walk: BoundedRandomWalk,
    jitter_counter: u32,
    /// Held walk value between advances (the walk steps every
    /// `JITTER_WALK_INTERVAL` samples, not every sample).
    jitter_value: f32,
}

impl BbdProto {
    pub fn new(
        input_poles: impl IntoIterator<Item = Complex32>,
        input_residues: impl IntoIterator<Item = Complex32>,
        output_poles: impl IntoIterator<Item = Complex32>,
        output_residues: impl IntoIterator<Item = Complex32>,
        stages: usize,
        host_sample_rate: f32,
    ) -> Self {
        let clock = BbdClock::new(host_sample_rate);
        let input =
            ContinuousPoleBank::new(input_poles, input_residues, host_sample_rate);
        let output =
            ContinuousPoleBank::new(output_poles, output_residues, host_sample_rate);
        let read_events = Vec::with_capacity(16);
        Self {
            clock,
            banks: Banks::Aos { input, output },
            buckets: vec![0.0; stages + 1],
            buffer_ptr: 0,
            last_bucket_read: 0.0,
            read_events,
            saturation_drive: 0.0,
            saturation_inv_drive: 1.0,
            stages,
            smoothing_interval: AOS_PLACEHOLDER_INTERVAL,
            inv_smoothing_interval: 1.0 / AOS_PLACEHOLDER_INTERVAL as f32,
            bbd_ts_cur: 0.0,
            bbd_ts_target: 0.0,
            bbd_ts_step: 0.0,
            ramp_samples_remaining: 0,
            has_delay_set: false,
            jitter_amount: 0.0,
            jitter_walk: BoundedRandomWalk::new(0x1BBD_0001, JITTER_WALK_STEP),
            jitter_counter: 0,
            jitter_value: 0.0,
        }
    }

    /// Conjugate-pair variant: each supplied pole represents a
    /// conjugate pair; the bank stores only the halves and doubles
    /// the real output internally. Uses a SoA pole layout and an
    /// incremental phasor so that per-sample `exp()` calls are
    /// eliminated for uniform-Δτ tick and segment increments.
    ///
    /// `smoothing_interval` sets the number of samples between
    /// expected [`Self::set_delay`] calls; the filter linearly
    /// interpolates `bbd_ts` and per-pole `alpha` between them.
    /// Expected to be a power of two so the client can gate the call
    /// with `counter & (interval - 1) == 0`. Chosen at construction
    /// time as a function of sample rate and not changed thereafter.
    #[allow(clippy::too_many_arguments)]
    pub fn new_conjugate_pairs(
        input_pole_pairs: impl IntoIterator<Item = Complex32>,
        input_residue_pairs: impl IntoIterator<Item = Complex32>,
        output_pole_pairs: impl IntoIterator<Item = Complex32>,
        output_residue_pairs: impl IntoIterator<Item = Complex32>,
        stages: usize,
        host_sample_rate: f32,
        smoothing_interval: u32,
    ) -> Self {
        let smoothing_interval = smoothing_interval.max(1);
        let clock = BbdClock::new(host_sample_rate);
        let input = ConjPairPoleBankSoa::new(
            input_pole_pairs,
            input_residue_pairs,
            host_sample_rate,
        );
        let output = ConjPairPoleBankSoa::new(
            output_pole_pairs,
            output_residue_pairs,
            host_sample_rate,
        );
        let scratch_n = input.len().max(output.len());
        let read_events = Vec::with_capacity(16);
        Self {
            clock,
            banks: Banks::Soa {
                input,
                output,
                phi_re: vec![0.0; scratch_n],
                phi_im: vec![0.0; scratch_n],
            },
            buckets: vec![0.0; stages + 1],
            buffer_ptr: 0,
            last_bucket_read: 0.0,
            read_events,
            saturation_drive: 0.0,
            saturation_inv_drive: 1.0,
            stages,
            smoothing_interval,
            inv_smoothing_interval: 1.0 / smoothing_interval as f32,
            bbd_ts_cur: 0.0,
            bbd_ts_target: 0.0,
            bbd_ts_step: 0.0,
            ramp_samples_remaining: 0,
            has_delay_set: false,
            jitter_amount: 0.0,
            jitter_walk: BoundedRandomWalk::new(0x1BBD_0001, JITTER_WALK_STEP),
            jitter_counter: 0,
            jitter_value: 0.0,
        }
    }

    /// Set bucket-write saturation drive. `0.0` (default) disables.
    /// Matches the hook `bbd::Bbd` exposes via `BbdDevice`.
    pub fn set_saturation_drive(&mut self, drive: f32) {
        self.saturation_drive = drive.max(0.0);
        self.saturation_inv_drive = if self.saturation_drive > 0.0 {
            1.0 / self.saturation_drive
        } else {
            1.0
        };
    }

    pub fn smoothing_interval(&self) -> u32 {
        self.smoothing_interval
    }

    /// Clock-jitter amount in `[0, 1]`. `0.0` is bit-identical to a
    /// non-jittered build — the random walk is not advanced and the
    /// clock sees the clean ramped `bbd_ts`.
    pub fn set_jitter_amount(&mut self, amount: f32) {
        let new_amount = amount.clamp(0.0, 1.0);
        if new_amount == 0.0 && self.jitter_amount > 0.0 && self.has_delay_set {
            // Realign the clock to the un-jittered `bbd_ts_cur` so the
            // tail of this session doesn't run at a permanently skewed
            // rate.
            self.clock.set_bbd_ts(self.bbd_ts_cur);
        }
        self.jitter_amount = new_amount;
    }

    /// Seed the jitter random walk. Call once at construction time (or
    /// whenever a module wants its BBDs to decorrelate) so per-instance
    /// BBDs wander independently.
    pub fn set_jitter_seed(&mut self, seed: u32) {
        self.jitter_walk = BoundedRandomWalk::new(seed, JITTER_WALK_STEP);
        self.jitter_counter = 0;
        self.jitter_value = 0.0;
    }

    /// Set the target delay. On the SoA path, the first call snaps
    /// immediately; subsequent calls schedule a linear ramp of
    /// `bbd_ts` and per-pole `alpha` over `smoothing_interval`
    /// samples. Expected call cadence from the client is once every
    /// `smoothing_interval` samples — matching the standard Periodic
    /// mechanism used for module parameter updates — but calls at
    /// other rates are safe (they just restart the ramp).
    pub fn set_delay(&mut self, delay_seconds: f32) {
        // Clamp delay using the same floor the clock applies so cur
        // and clock stay in lock-step.
        let bbd_ts_target = delay_seconds.max(1.0e-5) / (2.0 * self.stages as f32);
        let host_ts = self.clock.host_ts();
        let clock_floor = host_ts * 0.01;
        let bbd_ts_target = bbd_ts_target.max(clock_floor);

        self.bbd_ts_target = bbd_ts_target;

        match &mut self.banks {
            Banks::Aos { .. } => {
                // Legacy path: no per-pole smoothing, snap the clock.
                self.clock.set_bbd_ts(bbd_ts_target);
                self.bbd_ts_cur = bbd_ts_target;
                self.bbd_ts_step = 0.0;
                self.ramp_samples_remaining = 0;
                self.has_delay_set = true;
            }
            Banks::Soa { input, output, .. } => {
                let delta_tau_target = 2.0 * bbd_ts_target / host_ts;
                if !self.has_delay_set {
                    // First call — snap clock and alpha; no ramp.
                    self.clock.set_bbd_ts(bbd_ts_target);
                    self.bbd_ts_cur = bbd_ts_target;
                    self.bbd_ts_step = 0.0;
                    self.ramp_samples_remaining = 0;
                    input.snap_tick_delta_tau(delta_tau_target);
                    output.snap_tick_delta_tau(delta_tau_target);
                    self.has_delay_set = true;
                } else {
                    let inv = self.inv_smoothing_interval;
                    self.bbd_ts_step = (bbd_ts_target - self.bbd_ts_cur) * inv;
                    self.ramp_samples_remaining = self.smoothing_interval;
                    input.target_tick_delta_tau(delta_tau_target, inv);
                    output.target_tick_delta_tau(delta_tau_target, inv);
                }
            }
        }
    }

    pub fn reset(&mut self) {
        for b in self.buckets.iter_mut() {
            *b = 0.0;
        }
        self.buffer_ptr = 0;
        self.last_bucket_read = 0.0;
        self.read_events.clear();
        match &mut self.banks {
            Banks::Aos { input, output } => {
                input.reset();
                output.reset();
            }
            Banks::Soa { input, output, .. } => {
                input.reset();
                output.reset();
            }
        }
        self.clock.reset();
        // Clear any in-flight ramp. `bbd_ts_cur` / alpha remain at
        // their last values; a subsequent `set_delay` will ramp from
        // there (or snap, if it is the first call after construction).
        self.bbd_ts_step = 0.0;
        self.ramp_samples_remaining = 0;
    }

    pub fn process(&mut self, input: f32) -> f32 {
        // Split per-arm so each variant's lifetime over `&mut self.banks`
        // is bounded; the dispatch here is the only place that knows
        // about both arms.
        match &mut self.banks {
            Banks::Aos { .. } => Self::process_aos(
                &mut self.banks,
                &mut self.buckets,
                &mut self.buffer_ptr,
                &mut self.read_events,
                &mut self.last_bucket_read,
                &mut self.clock,
                &mut self.jitter_walk,
                &mut self.jitter_counter,
                &mut self.jitter_value,
                self.jitter_amount,
                self.bbd_ts_cur,
                self.saturation_drive,
                self.saturation_inv_drive,
                input,
            ),
            Banks::Soa { .. } => Self::process_soa(
                &mut self.banks,
                &mut self.buckets,
                &mut self.buffer_ptr,
                &mut self.read_events,
                &mut self.last_bucket_read,
                &mut self.clock,
                &mut self.jitter_walk,
                &mut self.jitter_counter,
                &mut self.jitter_value,
                self.jitter_amount,
                &mut self.bbd_ts_cur,
                &mut self.bbd_ts_step,
                self.bbd_ts_target,
                &mut self.ramp_samples_remaining,
                self.saturation_drive,
                self.saturation_inv_drive,
                input,
            ),
        }
    }

    /// AoS path: snap-clock + jitter, no ramp/phasor state. See
    /// `process_soa` for the SoA path that adds delay-smoothing ramp
    /// and the incremental phasor scratch.
    #[allow(clippy::too_many_arguments)]
    fn process_aos(
        banks: &mut Banks,
        buckets: &mut [f32],
        buffer_ptr: &mut usize,
        read_events: &mut Vec<(f32, f32)>,
        last_bucket_read: &mut f32,
        clock: &mut BbdClock,
        jitter_walk: &mut BoundedRandomWalk,
        jitter_counter: &mut u32,
        jitter_value: &mut f32,
        jitter_amount: f32,
        bbd_ts_cur: f32,
        sat: f32,
        inv_sat: f32,
        input: f32,
    ) -> f32 {
        let (input_bank, output_bank) = match banks {
            Banks::Aos { input, output } => (input, output),
            Banks::Soa { .. } => unreachable!("process_aos called on SoA banks"),
        };
        let len = buckets.len();
        let mut ptr = *buffer_ptr;
        read_events.clear();

        // Apply jitter after any ramp adjustment (AoS path snaps the
        // clock in `set_delay`, so no ramp work happens in process —
        // jitter just overwrites the static clock).
        if jitter_amount > 0.0 {
            if *jitter_counter == 0 {
                *jitter_value = jitter_walk.advance();
            }
            *jitter_counter = (*jitter_counter + 1) % JITTER_WALK_INTERVAL;
            let factor = 1.0 + *jitter_value * jitter_amount * JITTER_MAX_DEPTH;
            clock.set_bbd_ts(bbd_ts_cur * factor);
        }

        let ib = &*input_bank;
        clock.step(|tick| match tick.phase {
            TickPhase::Write => {
                let raw = ib.evaluate(tick.tau, input);
                let charge = if sat > 0.0 {
                    fast_tanh(sat * raw) * inv_sat
                } else {
                    raw
                };
                buckets[ptr] = charge;
                ptr += 1;
                if ptr == len {
                    ptr = 0;
                }
            }
            TickPhase::Read => {
                read_events.push((tick.tau, buckets[ptr]));
            }
        });
        *buffer_ptr = ptr;
        input_bank.advance(input);

        let mut last_tau = 0.0_f32;
        let mut current_bucket = *last_bucket_read;
        for &(tau, new_bucket) in read_events.iter() {
            let dtau = tau - last_tau;
            if dtau > 0.0 {
                output_bank.advance_by(dtau, current_bucket);
            }
            last_tau = tau;
            current_bucket = new_bucket;
        }
        let dtau_tail = 1.0 - last_tau;
        if dtau_tail > 0.0 {
            output_bank.advance_by(dtau_tail, current_bucket);
        }
        *last_bucket_read = current_bucket;
        output_bank.real_output()
    }

    /// SoA path: delay-smoothing ramp + jitter on top, plus the
    /// incremental phasor reuse on both the Write and the
    /// intra-sample output segments. The first Write fills `phi` via
    /// `exp`; subsequent Writes multiply by the tick-alpha cached on
    /// the bank.
    #[allow(clippy::too_many_arguments)]
    fn process_soa(
        banks: &mut Banks,
        buckets: &mut [f32],
        buffer_ptr: &mut usize,
        read_events: &mut Vec<(f32, f32)>,
        last_bucket_read: &mut f32,
        clock: &mut BbdClock,
        jitter_walk: &mut BoundedRandomWalk,
        jitter_counter: &mut u32,
        jitter_value: &mut f32,
        jitter_amount: f32,
        bbd_ts_cur: &mut f32,
        bbd_ts_step: &mut f32,
        bbd_ts_target: f32,
        ramp_samples_remaining: &mut u32,
        sat: f32,
        inv_sat: f32,
        input: f32,
    ) -> f32 {
        let (input_bank, output_bank, phi_re, phi_im) = match banks {
            Banks::Soa { input, output, phi_re, phi_im } => {
                (input, output, phi_re, phi_im)
            }
            Banks::Aos { .. } => unreachable!("process_soa called on AoS banks"),
        };
        let len = buckets.len();
        let mut ptr = *buffer_ptr;
        read_events.clear();

        // Advance any in-flight delay-smoothing ramp one sample. bbd_ts
        // and per-pole alpha interpolate linearly toward their targets
        // over `smoothing_interval` samples.
        if *ramp_samples_remaining > 0 {
            *bbd_ts_cur += *bbd_ts_step;
            input_bank.advance_alpha_smoothing();
            output_bank.advance_alpha_smoothing();
            *ramp_samples_remaining -= 1;
            if *ramp_samples_remaining == 0 {
                // Snap to eliminate float accumulation drift.
                *bbd_ts_cur = bbd_ts_target;
                *bbd_ts_step = 0.0;
                input_bank.snap_alpha_to_target();
                output_bank.snap_alpha_to_target();
            }
            clock.set_bbd_ts(*bbd_ts_cur);
        }

        // Clock-jitter perturbation on top of the ramp. When amount=0
        // this block is skipped entirely, preserving bit-for-bit
        // equivalence to a non-jittered build.
        if jitter_amount > 0.0 {
            if *jitter_counter == 0 {
                *jitter_value = jitter_walk.advance();
            }
            *jitter_counter = (*jitter_counter + 1) % JITTER_WALK_INTERVAL;
            let factor = 1.0 + *jitter_value * jitter_amount * JITTER_MAX_DEPTH;
            clock.set_bbd_ts(*bbd_ts_cur * factor);
        }

        // Incremental-phasor state for Writes within this sample: the
        // first Write computes phi via exp; each subsequent Write
        // multiplies phi by the precomputed tick alpha — no exp on the
        // hot path.
        let mut have_phi = false;
        let ib = &*input_bank;
        let phi_re_s = &mut phi_re[..];
        let phi_im_s = &mut phi_im[..];

        clock.step(|tick| match tick.phase {
            TickPhase::Write => {
                if !have_phi {
                    ib.fill_phi(tick.tau, phi_re_s, phi_im_s);
                    have_phi = true;
                } else {
                    ib.step_phi(phi_re_s, phi_im_s);
                }
                let raw = ib.evaluate_with_phi(phi_re_s, phi_im_s, input);
                let charge = if sat > 0.0 {
                    fast_tanh(sat * raw) * inv_sat
                } else {
                    raw
                };
                buckets[ptr] = charge;
                ptr += 1;
                if ptr == len {
                    ptr = 0;
                }
            }
            TickPhase::Read => {
                read_events.push((tick.tau, buckets[ptr]));
            }
        });
        *buffer_ptr = ptr;
        input_bank.advance(input);

        // Output segments: first (variable Δτ) uses inline exp; middle
        // segments all share `2·bbd_ts/host_ts` so reuse the cached
        // alpha table as phi; tail uses inline exp again.
        let mut last_tau = 0.0_f32;
        let mut current_bucket = *last_bucket_read;
        let mut is_first = true;
        let nout = output_bank.len();
        for &(tau, new_bucket) in read_events.iter() {
            let dtau = tau - last_tau;
            if dtau > 0.0 {
                if is_first {
                    output_bank.advance_by(dtau, current_bucket);
                    is_first = false;
                } else {
                    output_bank
                        .copy_alpha_into(&mut phi_re[..nout], &mut phi_im[..nout]);
                    output_bank.advance_by_phi(
                        &phi_re[..nout],
                        &phi_im[..nout],
                        current_bucket,
                    );
                }
            }
            last_tau = tau;
            current_bucket = new_bucket;
        }
        let dtau_tail = 1.0 - last_tau;
        if dtau_tail > 0.0 {
            output_bank.advance_by(dtau_tail, current_bucket);
        }
        *last_bucket_read = current_bucket;
        output_bank.real_output()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Plausible analog anti-imaging / reconstruction filters — a
    /// pair of complex-conjugate pole pairs giving a ~4-pole lowpass.
    /// Residues chosen for approximate unity DC gain. Not from any
    /// specific chip; tuned generically for tests.
    fn demo_input_poles() -> Vec<Complex32> {
        // Cutoff ~9 kHz, two conjugate pairs at different Qs.
        vec![
            Complex32::new(-50_000.0, 40_000.0),
            Complex32::new(-50_000.0, -40_000.0),
            Complex32::new(-30_000.0, 55_000.0),
            Complex32::new(-30_000.0, -55_000.0),
        ]
    }

    /// Unit residues. `normalised_residues` rescales these so DC gain
    /// is exactly 1.0 for the chosen poles — that's why the raw values
    /// don't have to be carefully chosen.
    fn demo_input_residues() -> Vec<Complex32> {
        vec![Complex32::new(1.0, 0.0); 4]
    }

    fn dc_gain(poles: &[Complex32], residues: &[Complex32]) -> f32 {
        // DC gain of Σ r_k/(s - p_k) evaluated at s=0: Σ -r_k/p_k.
        let mut sum = Complex32::new(0.0, 0.0);
        for (p, r) in poles.iter().zip(residues.iter()) {
            sum += -*r / *p;
        }
        sum.re
    }

    fn normalised_residues(
        poles: &[Complex32],
        residues: &[Complex32],
    ) -> Vec<Complex32> {
        let g = dc_gain(poles, residues);
        residues.iter().map(|r| *r * (1.0 / g)).collect()
    }

    fn build(stages: usize) -> BbdProto {
        let ip = demo_input_poles();
        let ir = normalised_residues(&ip, &demo_input_residues());
        let op = ip.clone();
        let or = ir.clone();
        BbdProto::new(ip, ir, op, or, stages, SR)
    }

    #[test]
    fn silence_in_silence_out() {
        let mut b = build(256);
        b.set_delay(0.003);
        let mut peak = 0.0_f32;
        for _ in 0..((SR * 0.1) as usize) {
            peak = peak.max(b.process(0.0).abs());
        }
        assert!(peak < 1.0e-5, "silence leaked: {peak}");
    }

    #[test]
    fn impulse_appears_near_commanded_delay() {
        let mut b = build(256);
        let delay_ms = 4.0_f32;
        b.set_delay(delay_ms * 1e-3);
        b.process(1.0);
        let horizon = (SR * (delay_ms * 1e-3 + 0.02)) as usize;
        let mut peak_idx = 0;
        let mut peak_abs = 0.0_f32;
        for i in 1..horizon {
            let y = b.process(0.0).abs();
            if y > peak_abs {
                peak_abs = y;
                peak_idx = i;
            }
        }
        let commanded = (delay_ms * 1e-3 * SR) as usize;
        let window = (SR * 2e-3) as usize;
        assert!(
            peak_idx > commanded.saturating_sub(window) && peak_idx < commanded + window,
            "impulse peak {peak_idx}, commanded {commanded}"
        );
    }

    #[test]
    fn sustained_sine_has_no_slow_drift() {
        // Same invariant the main `bbd` module asserts: no sub-Hz
        // amplitude modulation from mis-phased sub-sample gain.
        let mut b = build(256);
        b.set_delay(0.003);
        let freq = 440.0_f32;
        let amp = 0.05_f32; // linear regime — residues not pole-fit, so
                            // leave headroom
        let warmup = (SR * 0.1) as usize;
        let win = (SR * 0.05) as usize;
        let total = (SR * 3.0) as usize;
        let mut wins: Vec<f32> = Vec::new();
        let mut cur = 0.0_f32;
        for i in 0..total {
            let t = i as f32 / SR;
            let x = amp * (std::f32::consts::TAU * freq * t).sin();
            let y = b.process(x);
            if i >= warmup {
                cur = cur.max(y.abs());
                if (i - warmup + 1).is_multiple_of(win) {
                    wins.push(cur);
                    cur = 0.0;
                }
            }
        }
        let min = wins.iter().copied().fold(f32::INFINITY, f32::min);
        let max = wins.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let db = 20.0 * (max / min).log10();
        assert!(db < 1.0, "peaks drift {db:.2} dB (min {min:.4}, max {max:.4})");
    }

    #[test]
    fn long_delay_exhibits_image_folding() {
        // With a long delay the BBD clock drops below host Nyquist.
        // An input sine above clock/2 but well under host Nyquist
        // should get aliased — appear at a lower frequency in the
        // output. This is the behaviour H-P captures and a host-rate
        // simple-cascade BBD misses.
        //
        // Setup: 80 ms delay, 1024 stages → clock ≈ 25.6 kHz,
        // so clock/2 ≈ 12.8 kHz. Drive at 15 kHz — above clock/2,
        // within host passband. Expect a fold-back component.
        let mut b = build(1024);
        b.set_delay(0.080);
        let freq = 15_000.0_f32;
        let amp = 0.05_f32;
        // Warm up past transport delay.
        for _ in 0..((SR * 0.2) as usize) {
            let i = 0_usize;
            let _ = b.process(amp * (std::f32::consts::TAU * freq * (i as f32 / SR)).sin());
        }
        // Collect 100 ms of output.
        let n = (SR * 0.1) as usize;
        let mut samples = Vec::with_capacity(n);
        let base = (SR * 0.2) as usize;
        for i in 0..n {
            let t = (base + i) as f32 / SR;
            let x = amp * (std::f32::consts::TAU * freq * t).sin();
            samples.push(b.process(x));
        }
        // Bandpass check via DFT at the folded frequency
        // f_alias = |f - clock| = |15000 - 25600| = 10600 Hz.
        let clock_rate_hz = 2.0 * 1024.0 / 0.080;
        let f_alias = (freq - clock_rate_hz).abs();
        let (energy_original, energy_alias) =
            narrowband_energy(&samples, SR, freq, f_alias);
        // Alias component must be at least a few dB above the
        // passed-through component — if it weren't, the prototype
        // would be behaving like a plain host-rate LP filter, not
        // a BBD sampler.
        assert!(
            energy_alias > 0.05 * energy_original,
            "no image-fold energy: alias {energy_alias:.4}, original {energy_original:.4}"
        );
    }

    // ─── Matches-reference tests vs `bbd::Bbd` ────────────────────────────

    /// Both implementations should locate an impulse's peak at
    /// approximately the commanded delay. They won't agree bit-exact —
    /// different filter topologies — but the delay timing is a
    /// topology-free invariant.
    #[test]
    fn impulse_peak_matches_reference_bbd() {
        use crate::bbd::{Bbd, BbdDevice};

        fn proto_time_to_peak(delay_s: f32) -> usize {
            let mut p = build(256);
            p.set_delay(delay_s);
            p.process(1.0);
            let horizon = (SR * (delay_s + 0.02)) as usize;
            let mut pi = 0;
            let mut pa = 0.0_f32;
            for i in 1..horizon {
                let y = p.process(0.0).abs();
                if y > pa {
                    pa = y;
                    pi = i;
                }
            }
            pi
        }
        fn bbd_time_to_peak(delay_s: f32) -> usize {
            let mut b = Bbd::new(&BbdDevice::BBD_256, SR);
            b.set_delay_seconds(delay_s);
            let horizon = (SR * (delay_s + 0.02)) as usize;
            let mut pi = 0;
            let mut pa = 0.0_f32;
            b.process(1.0);
            for i in 1..horizon {
                let y = b.process(0.0).abs();
                if y > pa {
                    pa = y;
                    pi = i;
                }
            }
            pi
        }
        for ms in [3.0_f32, 5.0, 8.0] {
            let p = proto_time_to_peak(ms * 1e-3);
            let b = bbd_time_to_peak(ms * 1e-3);
            let diff = (p as i32 - b as i32).unsigned_abs() as usize;
            let tol = (SR * 2e-3) as usize; // 2 ms group-delay tolerance
            assert!(
                diff < tol,
                "{ms} ms: proto peak {p}, bbd peak {b}, diff {diff}"
            );
        }
    }

    /// Both should pass DC near unity for small signals. DC gain is
    /// topology-free — the residues are normalised at construction,
    /// and `bbd::Bbd`'s cascade of unity-gain 1-pole LPs also preserves
    /// DC.
    #[test]
    fn dc_gain_matches_reference_bbd() {
        use crate::bbd::{Bbd, BbdDevice};
        let amp = 0.05_f32;
        let settle = (SR * 0.05) as usize;
        let n = (SR * 0.02) as usize;

        let mut p = build(256);
        p.set_delay(0.003);
        for _ in 0..settle { p.process(amp); }
        let mut p_sum = 0.0_f32;
        for _ in 0..n { p_sum += p.process(amp); }
        let p_gain = (p_sum / n as f32) / amp;

        let mut b = Bbd::new(&BbdDevice::BBD_256, SR);
        b.set_delay_seconds(0.003);
        for _ in 0..settle { b.process(amp); }
        let mut b_sum = 0.0_f32;
        for _ in 0..n { b_sum += b.process(amp); }
        let b_gain = (b_sum / n as f32) / amp;

        // Both must be within ±1 dB of unity, and within ±0.5 dB of
        // each other.
        assert!(
            p_gain > 0.89 && p_gain < 1.12,
            "proto DC gain {p_gain} outside ±1 dB"
        );
        assert!(
            b_gain > 0.89 && b_gain < 1.12,
            "bbd DC gain {b_gain} outside ±1 dB"
        );
        let rel_db = 20.0 * (p_gain / b_gain).log10().abs();
        assert!(
            rel_db < 0.5,
            "proto and bbd DC gains differ by {rel_db:.2} dB (p={p_gain}, b={b_gain})"
        );
    }

    /// Both must be stable on a sustained passband sine — neither
    /// drifts. This is the invariant that broke the older BBD port.
    #[test]
    fn sustained_sine_both_stable() {
        use crate::bbd::{Bbd, BbdDevice};
        let freq = 440.0_f32;
        let amp = 0.05_f32;
        let n = (SR * 1.0) as usize;
        let settle = (SR * 0.1) as usize;

        let mut p = build(256);
        p.set_delay(0.003);
        let mut b = Bbd::new(&BbdDevice::BBD_256, SR);
        b.set_delay_seconds(0.003);

        let mut p_peak = 0.0_f32;
        let mut b_peak = 0.0_f32;
        for i in 0..n {
            let t = i as f32 / SR;
            let x = amp * (std::f32::consts::TAU * freq * t).sin();
            let py = p.process(x);
            let by = b.process(x);
            if i > settle {
                p_peak = p_peak.max(py.abs());
                b_peak = b_peak.max(by.abs());
            }
        }
        assert!(p_peak > 0.01 && p_peak < 0.1, "proto peak {p_peak} implausible");
        assert!(b_peak > 0.01 && b_peak < 0.1, "bbd peak {b_peak} implausible");
    }

    /// DFT-at-two-frequencies helper. Returns (|X(f_a)|, |X(f_b)|).
    fn narrowband_energy(x: &[f32], sr: f32, f_a: f32, f_b: f32) -> (f32, f32) {
        let mut sum_a_re = 0.0_f32;
        let mut sum_a_im = 0.0_f32;
        let mut sum_b_re = 0.0_f32;
        let mut sum_b_im = 0.0_f32;
        for (i, &v) in x.iter().enumerate() {
            let t = i as f32 / sr;
            let omega_a = std::f32::consts::TAU * f_a * t;
            let omega_b = std::f32::consts::TAU * f_b * t;
            sum_a_re += v * omega_a.cos();
            sum_a_im -= v * omega_a.sin();
            sum_b_re += v * omega_b.cos();
            sum_b_im -= v * omega_b.sin();
        }
        let n = x.len() as f32;
        let mag_a = (sum_a_re * sum_a_re + sum_a_im * sum_a_im).sqrt() / n;
        let mag_b = (sum_b_re * sum_b_re + sum_b_im * sum_b_im).sqrt() / n;
        (mag_a, mag_b)
    }

    // ── Clock-jitter tests ────────────────────────────────────────────────

    #[test]
    fn jitter_zero_is_bit_identical() {
        // jitter_amount = 0 must not touch the clock, so the sequence is
        // identical to a BbdProto that never saw the jitter API at all.
        let mut a = build(256);
        let mut b = build(256);
        a.set_delay(0.004);
        b.set_delay(0.004);
        b.set_jitter_seed(0xDEAD_BEEF);
        b.set_jitter_amount(0.0);

        let mut rng: u32 = 12345;
        for _ in 0..(SR as usize / 10) {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (rng as i32 as f32) * (1.0 / 2_147_483_648.0);
            let ya = a.process(x);
            let yb = b.process(x);
            assert_eq!(
                ya.to_bits(),
                yb.to_bits(),
                "jitter=0 diverged from baseline: {ya} vs {yb}",
            );
        }
    }

    #[test]
    fn jitter_changes_output_trajectory() {
        // Jittered and clean runs with otherwise identical setup should
        // diverge audibly. RMS of the difference is a crude but reliable
        // "something happened" check that doesn't depend on getting a
        // spectral measurement right.
        let sr = SR;
        let f_in = 1_000.0_f32;
        let n = 8_192;

        let capture = |jitter: f32| -> Vec<f32> {
            let mut b = build(1024);
            b.set_delay(0.010);
            b.set_jitter_seed(0xCAFE_BABE);
            b.set_jitter_amount(jitter);
            for i in 0..n {
                let t = i as f32 / sr;
                b.process((2.0 * std::f32::consts::PI * f_in * t).sin());
            }
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let t = (i + n) as f32 / sr;
                out.push(b.process((2.0 * std::f32::consts::PI * f_in * t).sin()));
            }
            out
        };

        let clean = capture(0.0);
        let jittered = capture(1.0);

        let mut sq_diff = 0.0_f64;
        let mut sq_clean = 0.0_f64;
        for (c, j) in clean.iter().zip(jittered.iter()) {
            sq_diff += ((c - j) as f64).powi(2);
            sq_clean += (*c as f64).powi(2);
        }
        let rms_diff = (sq_diff / clean.len() as f64).sqrt();
        let rms_clean = (sq_clean / clean.len() as f64).sqrt();
        let ratio = rms_diff / rms_clean.max(1e-9);
        // Expect jitter to move the output by at least a few percent of
        // the signal RMS. A clean sine through a fixed delay has a
        // constant-amplitude output; any meaningful delay wobble shows
        // up as non-trivial sample-by-sample divergence.
        assert!(
            ratio > 0.02,
            "jitter barely perturbed output: rms_diff/rms_clean = {ratio}",
        );
    }

    #[test]
    fn jitter_output_stays_bounded() {
        let mut b = build(1024);
        b.set_delay(0.020);
        b.set_jitter_seed(0x1234_5678);
        b.set_jitter_amount(1.0);
        let mut peak = 0.0_f32;
        for n in 0..(2 * SR as usize) {
            let x = if (n / 64) % 2 == 0 { 0.8 } else { -0.8 };
            let y = b.process(x);
            assert!(y.is_finite(), "non-finite at n={n}");
            peak = peak.max(y.abs());
        }
        assert!(peak < 4.0, "jittered output blew up: peak={peak}");
    }

    #[test]
    fn jitter_seeds_decorrelate() {
        // Two BBDs with the same delay but different jitter seeds should
        // produce different outputs under identical input (their clocks
        // wander independently).
        let mut a = build(512);
        let mut b = build(512);
        a.set_delay(0.006);
        b.set_delay(0.006);
        a.set_jitter_seed(0x1111_1111);
        b.set_jitter_seed(0x2222_2222);
        a.set_jitter_amount(1.0);
        b.set_jitter_amount(1.0);

        let sr = SR;
        let mut differ = 0;
        for i in 0..(sr as usize / 2) {
            let t = i as f32 / sr;
            let x = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let ya = a.process(x);
            let yb = b.process(x);
            if (ya - yb).abs() > 1.0e-4 {
                differ += 1;
            }
        }
        assert!(
            differ > (sr as usize / 2) / 10,
            "jitter seeds did not decorrelate (only {differ} differing samples)",
        );
    }
}
