//! Bucket-brigade-device (BBD) model.
//!
//! Uses the clean-room sub-sample-evaluated prototype from
//! [`crate::bbd_proto`] as its engine: a BBD clock yields write/read
//! ticks at their exact sub-sample instants, the input filter bank is
//! evaluated at each write-tick `τ`, and the output filter bank is
//! evolved through held-value segments between read ticks. Gives BBD-
//! clock image folding at long delays and a stable passband at short
//! delays.
//!
//! The filter shapes here are a plausible analog anti-imaging /
//! reconstruction design — two conjugate-pole pairs per side, residues
//! normalised for unit DC gain. Not a specific chip; tuned generically
//! for character and stability.
//!
//! # Real-time safety
//!
//! All buffers allocated in [`Bbd::new`]. [`Bbd::process`] and
//! [`Bbd::set_delay_seconds`] perform no allocations.

#[cfg(test)]
mod tests;

use crate::bbd_filter_proto::Complex32;
use crate::bbd_proto::BbdProto;

#[derive(Clone, Copy, Debug)]
pub struct BbdDevice {
    pub stages: usize,
    /// Soft-saturation drive on bucket writes. `0.0` disables.
    pub saturation_drive: f32,
}

impl BbdDevice {
    pub const BBD_256: Self = Self { stages: 256, saturation_drive: 1.2 };
    pub const BBD_1024: Self = Self { stages: 1024, saturation_drive: 1.2 };
    pub const BBD_4096: Self = Self { stages: 4096, saturation_drive: 1.2 };
}

/// Input / output filter pole set. Two well-damped conjugate-pole
/// pairs (Q ≈ 0.3) giving a non-peaking ~4-pole lowpass rolling off
/// from ~6 kHz. Damped by design so that the BBD's combined input ×
/// output transfer stays below unity everywhere — this keeps feedback
/// networks (FDN reverbs, self-feedback delays) from gaining at any
/// in-band frequency. Not a specific chip; tuned generically.
///
/// Returns one pole per conjugate pair; the bank adds the conjugate
/// twins implicitly.
fn default_pole_pairs() -> [Complex32; 2] {
    [
        Complex32::new(-30_000.0, 20_000.0),
        Complex32::new(-50_000.0, 30_000.0),
    ]
}

/// Residues (one per pair) normalised so the filter's DC gain
/// `2·Σ Re(-r/p)` over the halves is exactly 1.
fn normalised_pair_residues(poles: &[Complex32; 2]) -> [Complex32; 2] {
    let raw = [Complex32::new(1.0, 0.0); 2];
    let mut g = 0.0_f32;
    for (p, r) in poles.iter().zip(raw.iter()) {
        let q = -*r / *p;
        g += 2.0 * q.re;
    }
    let inv_g = 1.0 / g;
    [raw[0] * inv_g, raw[1] * inv_g]
}

/// Bucket-brigade delay line.
pub struct Bbd {
    proto: BbdProto,
    delay_s: f32,
    stages: usize,
}

/// Power-of-two smoothing interval targeting ~333 μs between delay
/// updates (the "Periodic tick" stride clients use when modulating
/// delay from CV). 16 at 48 kHz; scales up at higher sample rates.
fn smoothing_interval_for(sample_rate: f32) -> u32 {
    let raw = (sample_rate / 3000.0).max(1.0) as u32;
    raw.next_power_of_two()
}

impl Bbd {
    /// Construct with a sample-rate-derived smoothing interval. Use
    /// when the BBD's delay is driven internally (e.g. by a kernel-
    /// owned LFO in chorus/flanger), so the stride can be finer than
    /// the module's Periodic cadence.
    pub fn new(device: &BbdDevice, sample_rate: f32) -> Self {
        Self::new_with_smoothing_interval(
            device,
            sample_rate,
            smoothing_interval_for(sample_rate),
        )
    }

    /// Construct with an explicit smoothing interval — for modules
    /// that drive `set_delay_seconds` from `PeriodicUpdate`, pass
    /// `env.periodic_update_interval` so the BBD's ramp aligns with
    /// the Periodic callback cadence.
    pub fn new_with_smoothing_interval(
        device: &BbdDevice,
        sample_rate: f32,
        smoothing_interval: u32,
    ) -> Self {
        let poles = default_pole_pairs();
        let residues = normalised_pair_residues(&poles);
        let mut proto = BbdProto::new_conjugate_pairs(
            poles,
            residues,
            poles,
            residues,
            device.stages,
            sample_rate,
            smoothing_interval,
        );
        proto.set_saturation_drive(device.saturation_drive);
        let mut me = Self { proto, delay_s: 0.0, stages: device.stages };
        me.set_delay_seconds(0.003);
        me
    }

    pub fn set_delay_seconds(&mut self, delay: f32) {
        let delay = delay.max(1.0e-5);
        if (delay - self.delay_s).abs() < 1.0e-9 {
            return;
        }
        self.delay_s = delay;
        self.proto.set_delay(delay);
    }

    pub fn process(&mut self, input: f32) -> f32 {
        self.proto.process(input)
    }

    pub fn reset(&mut self) {
        self.proto.reset();
    }

    pub fn delay_seconds(&self) -> f32 {
        self.delay_s
    }

    pub fn stages(&self) -> usize {
        self.stages
    }

    /// Smoothing interval in samples. Clients modulating delay should
    /// call [`Self::set_delay_seconds`] once every `interval` samples
    /// for best perf (one `exp()` per pole per call, amortised). A
    /// power of two — use `counter & (interval - 1) == 0` to gate.
    pub fn smoothing_interval(&self) -> u32 {
        self.proto.smoothing_interval()
    }

    /// Set clock-jitter amount in `[0, 1]`. `0.0` is bit-identical to a
    /// non-jittered build — modules that never touch jitter incur no
    /// runtime cost beyond a per-sample branch on `amount > 0`.
    pub fn set_jitter_amount(&mut self, amount: f32) {
        self.proto.set_jitter_amount(amount);
    }

    /// Seed the jitter random walk so multiple BBDs in the same module
    /// (e.g. dual-stage chorus, FDN reverb) wander independently.
    pub fn set_jitter_seed(&mut self, seed: u32) {
        self.proto.set_jitter_seed(seed);
    }
}
