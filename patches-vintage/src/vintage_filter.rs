//! Shared scaffolding for the four ladder/OTA-VCF modules
//! ([`crate::vladder`], [`crate::vpoly_ladder`], [`crate::vota_vcf`],
//! [`crate::vota_poly_vcf`]).
//!
//! Each of those modules wraps a `patches-dsp` filter kernel and bolts
//! on identical port/parameter plumbing: V/oct cutoff CV, optional
//! `GLOBAL_DRIFT` backplane modulation, the `apply_static` vs
//! `begin_ramp` cadence, and the `interval_recip` derivation. This
//! module factors all of that out into one generic core
//! (`VintageVcfMonoCore` / `VintageVcfPolyCore`) parameterised over a
//! kernel-adapter trait. The four `Module` impls shrink to
//! descriptor templates + thin forwarders.

use patches_sdk::{
    AudioEnvironment, CablePool, InputPort, MonoInput, MonoOutput, OutputPort, PolyInput,
    PolyOutput, GLOBAL_DRIFT,
};

use patches_dsp::{
    LadderCoeffs, LadderKernel, LadderVariant, OtaLadderCoeffs, OtaLadderKernel, OtaPoles,
    PolyLadderKernel, PolyOtaLadderKernel,
};

pub(crate) const CUTOFF_MIN: f32 = 20.0;
pub(crate) const CUTOFF_MAX: f32 = 20_000.0;
pub(crate) const DRIVE_MAX: f32 = 4.0;
/// Max cents of cutoff detune at `drift_amount = 1.0, drift = ±1.0`.
/// Only OTA-VCF modules expose `drift_amount`; the ladder pair always
/// passes `drift_amount = 0.0`.
pub(crate) const MAX_DRIFT_CENTS: f32 = 25.0;

/// Apply CV (V/oct) + drift (V/oct, scaled by `drift_amount`) to the
/// base cutoff in Hz and clamp into the BBD-usable range.
#[inline]
pub(crate) fn effective_cutoff(
    base_hz: f32,
    cv_voct: f32,
    drift_sample: f32,
    drift_amount: f32,
    sample_rate: f32,
) -> f32 {
    let drift_voct = drift_sample * drift_amount * (MAX_DRIFT_CENTS / 1200.0);
    let total_voct = cv_voct + drift_voct;
    (base_hz * (2.0f32).powf(total_voct)).clamp(CUTOFF_MIN, sample_rate * 0.45)
}

// ── Mono trait + core ───────────────────────────────────────────────────────

pub(crate) trait MonoVcfKernel: Sized {
    type Voicing: Copy;

    fn new_with_params(
        sr: f32,
        voicing: Self::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self;

    fn apply_static_params(
        &mut self,
        sr: f32,
        voicing: Self::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    );

    fn begin_ramp_params(
        &mut self,
        sr: f32,
        voicing: Self::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        interval_recip: f32,
    );

    /// Hook for voicings that the kernel tracks separately from the
    /// coeffs (e.g. OTA `poles`). Default is no-op for kernels that
    /// bake voicing into the coeffs (e.g. ladder `variant`).
    fn on_voicing_changed(&mut self, _voicing: Self::Voicing) {}

    fn tick(&mut self, x: f32) -> f32;
}

pub(crate) struct VintageVcfMonoCore<K: MonoVcfKernel> {
    sample_rate: f32,
    interval_recip: f32,
    voicing: K::Voicing,
    cutoff: f32,
    resonance: f32,
    drive: f32,
    /// `0.0` for modules without drift. Set by `set_params` from
    /// the module's `drift_amount` (or always `0.0` if absent).
    drift_amount: f32,
    kernel: K,
    in_audio: MonoInput,
    in_cutoff_cv: MonoInput,
    in_global_drift: MonoInput,
    out_audio: MonoOutput,
    /// `false` until `set_ports` runs. `set_params` skips the
    /// `apply_static` call before this is set so we don't double-tap
    /// `kernel.set_static` (once during the initial parameter snapshot
    /// the framework pushes before wiring ports, and again during
    /// `set_ports`).
    ports_initialized: bool,
}

impl<K: MonoVcfKernel> VintageVcfMonoCore<K> {
    pub(crate) fn new(
        env: &AudioEnvironment,
        with_drift: bool,
        voicing: K::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self {
        let kernel = K::new_with_params(env.sample_rate, voicing, cutoff, resonance, drive);
        let in_global_drift = if with_drift {
            MonoInput::backplane(GLOBAL_DRIFT)
        } else {
            MonoInput::default()
        };
        Self {
            sample_rate: env.sample_rate,
            interval_recip: 1.0 / env.periodic_update_interval as f32,
            voicing,
            cutoff,
            resonance,
            drive,
            drift_amount: 0.0,
            kernel,
            in_audio: MonoInput::default(),
            in_cutoff_cv: MonoInput::default(),
            in_global_drift,
            out_audio: MonoOutput::default(),
            ports_initialized: false,
        }
    }

    fn apply_static_now(&mut self) {
        let eff = effective_cutoff(self.cutoff, 0.0, 0.0, 0.0, self.sample_rate);
        self.kernel
            .apply_static_params(self.sample_rate, self.voicing, eff, self.resonance, self.drive);
    }

    pub(crate) fn set_params(
        &mut self,
        voicing: K::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        drift_amount: f32,
    ) {
        self.voicing = voicing;
        self.cutoff = cutoff;
        self.resonance = resonance;
        self.drive = drive;
        self.drift_amount = drift_amount;
        self.kernel.on_voicing_changed(voicing);
        if self.ports_initialized
            && !self.in_cutoff_cv.is_connected()
            && self.drift_amount == 0.0
        {
            self.apply_static_now();
        }
    }

    pub(crate) fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_audio = MonoInput::from_ports(inputs, 0);
        self.in_cutoff_cv = MonoInput::from_ports(inputs, 1);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
        self.ports_initialized = true;
        if !self.in_cutoff_cv.is_connected() && self.drift_amount == 0.0 {
            self.apply_static_now();
        }
    }

    pub(crate) fn process(&mut self, pool: &mut CablePool<'_>) {
        let x = pool.read_mono(&self.in_audio);
        let y = self.kernel.tick(x);
        pool.write_mono(&self.out_audio, y);
    }

    pub(crate) fn periodic_update(&mut self, pool: &CablePool<'_>) {
        let cv_connected = self.in_cutoff_cv.is_connected();
        let drift_active = self.drift_amount > 0.0;
        if !cv_connected && !drift_active {
            return;
        }
        let cv = if cv_connected {
            pool.read_mono(&self.in_cutoff_cv)
        } else {
            0.0
        };
        let drift = if drift_active {
            pool.read_mono(&self.in_global_drift)
        } else {
            0.0
        };
        let eff = effective_cutoff(self.cutoff, cv, drift, self.drift_amount, self.sample_rate);
        self.kernel.begin_ramp_params(
            self.sample_rate,
            self.voicing,
            eff,
            self.resonance,
            self.drive,
            self.interval_recip,
        );
    }
}

// ── Mono kernel adapters ────────────────────────────────────────────────────

impl MonoVcfKernel for LadderKernel {
    type Voicing = LadderVariant;

    fn new_with_params(
        sr: f32,
        voicing: LadderVariant,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self {
        Self::new_static(LadderCoeffs::new(cutoff, sr, resonance, drive, voicing))
    }

    fn apply_static_params(
        &mut self,
        sr: f32,
        voicing: LadderVariant,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) {
        self.set_static(LadderCoeffs::new(cutoff, sr, resonance, drive, voicing));
    }

    fn begin_ramp_params(
        &mut self,
        sr: f32,
        voicing: LadderVariant,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        interval_recip: f32,
    ) {
        self.begin_ramp(
            LadderCoeffs::new(cutoff, sr, resonance, drive, voicing),
            interval_recip,
        );
    }

    fn tick(&mut self, x: f32) -> f32 {
        LadderKernel::tick(self, x)
    }
}

impl MonoVcfKernel for OtaLadderKernel {
    type Voicing = OtaPoles;

    fn new_with_params(
        sr: f32,
        voicing: OtaPoles,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self {
        // `k` always scales by `OtaPoles::Four::k_max()` so the
        // resonance feel is consistent across 2/4-pole modes.
        let k = resonance * OtaPoles::Four.k_max();
        Self::new_static(OtaLadderCoeffs::new(cutoff, sr, k, drive), voicing)
    }

    fn apply_static_params(
        &mut self,
        sr: f32,
        _voicing: OtaPoles,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) {
        let k = resonance * OtaPoles::Four.k_max();
        self.set_static(OtaLadderCoeffs::new(cutoff, sr, k, drive));
    }

    fn begin_ramp_params(
        &mut self,
        sr: f32,
        _voicing: OtaPoles,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        interval_recip: f32,
    ) {
        let k = resonance * OtaPoles::Four.k_max();
        self.begin_ramp(OtaLadderCoeffs::new(cutoff, sr, k, drive), interval_recip);
    }

    fn on_voicing_changed(&mut self, voicing: OtaPoles) {
        self.set_poles(voicing);
    }

    fn tick(&mut self, x: f32) -> f32 {
        OtaLadderKernel::tick(self, x)
    }
}

// ── Poly trait + core ───────────────────────────────────────────────────────

pub(crate) trait PolyVcfKernel: Sized {
    type Voicing: Copy;

    fn new_with_params(
        sr: f32,
        voicing: Self::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self;

    fn apply_static_params(
        &mut self,
        sr: f32,
        voicing: Self::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    );

    #[allow(clippy::too_many_arguments)]
    fn begin_ramp_voice_params(
        &mut self,
        i: usize,
        sr: f32,
        voicing: Self::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        interval_recip: f32,
    );

    fn on_voicing_changed(&mut self, _voicing: Self::Voicing) {}

    fn tick_all(&mut self, audio: &[f32; 16], ramp: bool) -> [f32; 16];
}

pub(crate) struct VintageVcfPolyCore<K: PolyVcfKernel> {
    sample_rate: f32,
    interval_recip: f32,
    voicing: K::Voicing,
    cutoff: f32,
    resonance: f32,
    drive: f32,
    drift_amount: f32,
    kernel: K,
    in_audio: PolyInput,
    in_cutoff_cv: PolyInput,
    in_global_drift: MonoInput,
    out_audio: PolyOutput,
    /// See `VintageVcfMonoCore::ports_initialized`.
    ports_initialized: bool,
}

impl<K: PolyVcfKernel> VintageVcfPolyCore<K> {
    pub(crate) fn new(
        env: &AudioEnvironment,
        with_drift: bool,
        voicing: K::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self {
        let kernel = K::new_with_params(env.sample_rate, voicing, cutoff, resonance, drive);
        let in_global_drift = if with_drift {
            MonoInput::backplane(GLOBAL_DRIFT)
        } else {
            MonoInput::default()
        };
        Self {
            sample_rate: env.sample_rate,
            interval_recip: 1.0 / env.periodic_update_interval as f32,
            voicing,
            cutoff,
            resonance,
            drive,
            drift_amount: 0.0,
            kernel,
            in_audio: PolyInput::default(),
            in_cutoff_cv: PolyInput::default(),
            in_global_drift,
            out_audio: PolyOutput::default(),
            ports_initialized: false,
        }
    }

    fn apply_static_now(&mut self) {
        let eff = effective_cutoff(self.cutoff, 0.0, 0.0, 0.0, self.sample_rate);
        self.kernel
            .apply_static_params(self.sample_rate, self.voicing, eff, self.resonance, self.drive);
    }

    pub(crate) fn set_params(
        &mut self,
        voicing: K::Voicing,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        drift_amount: f32,
    ) {
        self.voicing = voicing;
        self.cutoff = cutoff;
        self.resonance = resonance;
        self.drive = drive;
        self.drift_amount = drift_amount;
        self.kernel.on_voicing_changed(voicing);
        if self.ports_initialized
            && !self.in_cutoff_cv.is_connected()
            && self.drift_amount == 0.0
        {
            self.apply_static_now();
        }
    }

    pub(crate) fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_audio = PolyInput::from_ports(inputs, 0);
        self.in_cutoff_cv = PolyInput::from_ports(inputs, 1);
        self.out_audio = PolyOutput::from_ports(outputs, 0);
        self.ports_initialized = true;
        if !self.in_cutoff_cv.is_connected() && self.drift_amount == 0.0 {
            self.apply_static_now();
        }
    }

    pub(crate) fn process(&mut self, pool: &mut CablePool<'_>) {
        if !self.out_audio.is_connected() {
            return;
        }
        let audio = if self.in_audio.is_connected() {
            pool.read_poly(&self.in_audio)
        } else {
            [0.0f32; 16]
        };
        let ramp = self.in_cutoff_cv.is_connected() || self.drift_amount > 0.0;
        let out = self.kernel.tick_all(&audio, ramp);
        pool.write_poly(&self.out_audio, out);
    }

    pub(crate) fn periodic_update(&mut self, pool: &CablePool<'_>) {
        let cv_connected = self.in_cutoff_cv.is_connected();
        let drift_active = self.drift_amount > 0.0;
        if !cv_connected && !drift_active {
            return;
        }
        let cv = if cv_connected {
            pool.read_poly(&self.in_cutoff_cv)
        } else {
            [0.0f32; 16]
        };
        let drift = if drift_active {
            pool.read_mono(&self.in_global_drift)
        } else {
            0.0
        };
        for (i, &v) in cv.iter().enumerate() {
            let eff = effective_cutoff(self.cutoff, v, drift, self.drift_amount, self.sample_rate);
            self.kernel.begin_ramp_voice_params(
                i,
                self.sample_rate,
                self.voicing,
                eff,
                self.resonance,
                self.drive,
                self.interval_recip,
            );
        }
    }
}

// ── Poly kernel adapters ────────────────────────────────────────────────────

impl PolyVcfKernel for PolyLadderKernel {
    type Voicing = LadderVariant;

    fn new_with_params(
        sr: f32,
        voicing: LadderVariant,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self {
        Self::new_static(LadderCoeffs::new(cutoff, sr, resonance, drive, voicing))
    }

    fn apply_static_params(
        &mut self,
        sr: f32,
        voicing: LadderVariant,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) {
        self.set_static(LadderCoeffs::new(cutoff, sr, resonance, drive, voicing));
    }

    fn begin_ramp_voice_params(
        &mut self,
        i: usize,
        sr: f32,
        voicing: LadderVariant,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        interval_recip: f32,
    ) {
        self.begin_ramp_voice(
            i,
            LadderCoeffs::new(cutoff, sr, resonance, drive, voicing),
            interval_recip,
        );
    }

    fn on_voicing_changed(&mut self, voicing: LadderVariant) {
        self.set_variant(voicing);
    }

    fn tick_all(&mut self, audio: &[f32; 16], ramp: bool) -> [f32; 16] {
        PolyLadderKernel::tick_all(self, audio, ramp)
    }
}

impl PolyVcfKernel for PolyOtaLadderKernel {
    type Voicing = OtaPoles;

    fn new_with_params(
        sr: f32,
        voicing: OtaPoles,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) -> Self {
        let k = resonance * OtaPoles::Four.k_max();
        Self::new_static(OtaLadderCoeffs::new(cutoff, sr, k, drive), voicing)
    }

    fn apply_static_params(
        &mut self,
        sr: f32,
        _voicing: OtaPoles,
        cutoff: f32,
        resonance: f32,
        drive: f32,
    ) {
        let k = resonance * OtaPoles::Four.k_max();
        self.set_static(OtaLadderCoeffs::new(cutoff, sr, k, drive));
    }

    fn begin_ramp_voice_params(
        &mut self,
        i: usize,
        sr: f32,
        _voicing: OtaPoles,
        cutoff: f32,
        resonance: f32,
        drive: f32,
        interval_recip: f32,
    ) {
        let k = resonance * OtaPoles::Four.k_max();
        self.begin_ramp_voice(i, OtaLadderCoeffs::new(cutoff, sr, k, drive), interval_recip);
    }

    fn on_voicing_changed(&mut self, voicing: OtaPoles) {
        self.set_poles(voicing);
    }

    fn tick_all(&mut self, audio: &[f32; 16], ramp: bool) -> [f32; 16] {
        PolyOtaLadderKernel::tick_all(self, audio, ramp)
    }
}
