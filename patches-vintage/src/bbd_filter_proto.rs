//! Sub-sample-evaluated continuous-time filter prototype.
//!
//! A single complex one-pole section `dx/dt = p·x + u(t)` with `u(t)`
//! piecewise-constant between host samples. State is advanced once
//! per host sample using the closed-form solution of the ODE; the
//! filter can also be evaluated at any sub-sample fraction `τ ∈ [0,1]`
//! without re-running anything — that's the piece the full H-P-style
//! BBD needs in order to sample its input at BBD clock moments that
//! don't align with host samples.
//!
//! Convention:
//! - `u[n]` is the input value held over `[n·Ts, (n+1)·Ts)`.
//! - `x[n] = y(n·Ts)` is the state at the start of sample `n`.
//! - `advance(u)` takes `u[n]` and rolls `x[n]` forward to `x[n+1]`.
//! - Before `advance`, `evaluate(τ, u)` gives `y(n·Ts + τ·Ts)`.
//!
//! Closed-form: with `φ(τ) = exp(p·τ·Ts)` and `ψ(τ) = (φ(τ)-1)/p`,
//! `evaluate(τ, u) = φ(τ)·x + ψ(τ)·u` and `advance(u)` is exactly
//! `evaluate(1, u)`. Both share the same formula so the stitch
//! between samples is analytical, not approximate.
//!
//! This module is a prototype — not wired into [`crate::bbd`] yet.

// Minimal complex-f32 helper — avoids pulling `num-complex` for one file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex32 {
    pub re: f32,
    pub im: f32,
}

impl Complex32 {
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }
    pub fn conj(self) -> Self {
        Self { re: self.re, im: -self.im }
    }
    pub fn exp(self) -> Self {
        let m = self.re.exp();
        let (s, c) = self.im.sin_cos();
        Self { re: m * c, im: m * s }
    }
    pub fn powf(self, b: f32) -> Self {
        let mag = (self.re * self.re + self.im * self.im).sqrt();
        let ang = self.im.atan2(self.re);
        let new_mag = mag.powf(b);
        let (s, c) = (ang * b).sin_cos();
        Self { re: new_mag * c, im: new_mag * s }
    }
    /// Multiplicative inverse `1/z`. Undefined at zero.
    pub fn inv(self) -> Self {
        let inv_d = 1.0 / (self.re * self.re + self.im * self.im);
        Self { re: self.re * inv_d, im: -self.im * inv_d }
    }
}

impl std::ops::Add for Complex32 {
    type Output = Self;
    fn add(self, o: Self) -> Self { Self { re: self.re + o.re, im: self.im + o.im } }
}
impl std::ops::Sub for Complex32 {
    type Output = Self;
    fn sub(self, o: Self) -> Self { Self { re: self.re - o.re, im: self.im - o.im } }
}
impl std::ops::Mul for Complex32 {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}
impl std::ops::Mul<f32> for Complex32 {
    type Output = Self;
    fn mul(self, s: f32) -> Self { Self { re: self.re * s, im: self.im * s } }
}
impl std::ops::Div for Complex32 {
    type Output = Self;
    fn div(self, o: Self) -> Self {
        let inv_d = 1.0 / (o.re * o.re + o.im * o.im);
        Self {
            re: (self.re * o.re + self.im * o.im) * inv_d,
            im: (self.im * o.re - self.re * o.im) * inv_d,
        }
    }
}
impl std::ops::Neg for Complex32 {
    type Output = Self;
    fn neg(self) -> Self { Self { re: -self.re, im: -self.im } }
}
impl std::ops::AddAssign for Complex32 {
    fn add_assign(&mut self, o: Self) { self.re += o.re; self.im += o.im; }
}

/// One continuous-time complex pole with closed-form sub-sample
/// evaluation. Real output is `re(evaluate(…))` in the bank sum.
#[derive(Clone, Debug)]
pub struct ContinuousPole {
    /// Continuous-time pole (rad/s). Typically Re(p) < 0 for stability.
    pole: Complex32,
    /// `1 / pole` — precomputed for ψ-formation on hot path.
    inv_pole: Complex32,
    /// `pole · host_ts` — precomputed so sub-sample φ eval is one scalar mul + exp.
    pole_ts: Complex32,
    /// `φ(1) = exp(p·Ts)` — per-host-sample state transition.
    pole_corr: Complex32,
    /// `ψ(1) = (φ(1) - 1) / p` — per-host-sample input response.
    psi1: Complex32,
    /// State `x[n] = y(n·Ts)`.
    x: Complex32,
}

impl ContinuousPole {
    pub fn new(pole: Complex32, sample_rate: f32) -> Self {
        let host_ts = 1.0 / sample_rate;
        let pole_ts = pole * host_ts;
        let pole_corr = pole_ts.exp();
        let inv_pole = pole.inv();
        let psi1 = Complex32 { re: pole_corr.re - 1.0, im: pole_corr.im } * inv_pole;
        Self {
            pole,
            inv_pole,
            pole_ts,
            pole_corr,
            psi1,
            x: Complex32::new(0.0, 0.0),
        }
    }

    pub fn pole(&self) -> Complex32 {
        self.pole
    }

    pub fn pole_corr(&self) -> Complex32 {
        self.pole_corr
    }

    pub fn state(&self) -> Complex32 {
        self.x
    }

    /// Closed-form `φ(τ) = exp(p · τ · Ts)` — the decay/rotation factor
    /// for sub-sample time `τ ∈ [0, 1]`. Used to compute an impulse's
    /// residual contribution at the end of the host sample:
    /// `contribution = φ(1 - τ) · impulse_value`.
    pub fn phi(&self, tau: f32) -> Complex32 {
        (self.pole_ts * tau).exp()
    }

    /// Replace the state — for tests and external drivers that bypass
    /// the normal `advance`/`evaluate` flow.
    pub fn set_state(&mut self, x: Complex32) {
        self.x = x;
    }

    pub fn reset(&mut self) {
        self.x = Complex32::new(0.0, 0.0);
    }

    /// Evaluate `y(n·Ts + τ·Ts)` given input `u` held for the current
    /// sample. `τ ∈ [0, 1]` — at `τ = 0` the state-contribution
    /// dominates; at `τ = 1` this matches the post-advance state.
    pub fn evaluate(&self, tau: f32, u: f32) -> Complex32 {
        let phi = (self.pole_ts * tau).exp();
        let psi = Complex32 { re: phi.re - 1.0, im: phi.im } * self.inv_pole;
        phi * self.x + psi * u
    }

    /// Roll state forward one host sample with input `u` held over
    /// `[n·Ts, (n+1)·Ts)`.
    pub fn advance(&mut self, u: f32) {
        self.x = self.pole_corr * self.x + self.psi1 * u;
    }

    /// Evolve state by a fraction `Δτ ∈ [0, 1]` of a host sample with
    /// `u` held constant throughout the interval. Closed-form:
    /// `x_new = φ(Δτ)·x + ψ(Δτ)·u`. Useful for output reconstruction
    /// where the input to the filter changes at sub-sample Read-tick
    /// boundaries.
    pub fn advance_by(&mut self, delta_tau: f32, u: f32) {
        let phi = (self.pole_ts * delta_tau).exp();
        let psi = Complex32 { re: phi.re - 1.0, im: phi.im } * self.inv_pole;
        self.x = phi * self.x + psi * u;
    }
}

/// A bank of complex one-poles with real residues summed at output.
/// Real poles must come as conjugate pairs for real-valued output.
#[derive(Clone, Debug)]
pub struct ContinuousPoleBank {
    poles: Vec<ContinuousPole>,
    residues: Vec<Complex32>,
}

impl ContinuousPoleBank {
    pub fn new(
        poles: impl IntoIterator<Item = Complex32>,
        residues: impl IntoIterator<Item = Complex32>,
        sample_rate: f32,
    ) -> Self {
        let poles: Vec<_> = poles
            .into_iter()
            .map(|p| ContinuousPole::new(p, sample_rate))
            .collect();
        let residues: Vec<_> = residues.into_iter().collect();
        assert_eq!(poles.len(), residues.len());
        Self { poles, residues }
    }

    /// `H(s) = Σ r_k / (s - p_k)` evaluated at sub-sample `τ`.
    pub fn evaluate(&self, tau: f32, u: f32) -> f32 {
        let mut sum = Complex32::new(0.0, 0.0);
        for (p, r) in self.poles.iter().zip(self.residues.iter()) {
            sum += *r * p.evaluate(tau, u);
        }
        sum.re
    }

    pub fn advance(&mut self, u: f32) {
        for p in self.poles.iter_mut() {
            p.advance(u);
        }
    }

    /// Evolve every pole by a sub-sample fraction with `u` held. See
    /// [`ContinuousPole::advance_by`].
    pub fn advance_by(&mut self, delta_tau: f32, u: f32) {
        for p in self.poles.iter_mut() {
            p.advance_by(delta_tau, u);
        }
    }

    pub fn reset(&mut self) {
        for p in self.poles.iter_mut() {
            p.reset();
        }
    }

    pub fn pole_count(&self) -> usize {
        self.poles.len()
    }

    pub fn poles(&self) -> &[ContinuousPole] {
        &self.poles
    }

    pub fn poles_mut(&mut self) -> &mut [ContinuousPole] {
        &mut self.poles
    }

    pub fn residues(&self) -> &[Complex32] {
        &self.residues
    }

    /// Sum `Σ r_k · x_k` (complex) and return the real part. Useful
    /// when the bank is driven externally via per-pole state
    /// manipulation rather than the built-in `advance(u)`.
    pub fn real_output(&self) -> f32 {
        let mut sum = Complex32::default();
        for (pole, r) in self.poles.iter().zip(self.residues.iter()) {
            sum += *r * pole.state();
        }
        sum.re
    }
}

/// Structure-of-arrays conjugate-pair pole bank with precomputed
/// per-pole `alpha = exp(pole_ts · Δτ_tick)` for incremental-phasor
/// evaluation across uniform sub-sample steps.
///
/// Stores one pole per conjugate pair; output is `2·Re(Σ r_k x_k)`
/// over the stored halves. All per-pole quantities live in parallel
/// `Vec<f32>`s so the tight per-sample loops are scalar-flat and
/// autovectorisable — no struct indirection, no complex-type ops.
///
/// `set_tick_delta_tau` recomputes the `alpha` table for a given
/// uniform tick increment, letting callers replace per-call `exp()`
/// with a cheap complex multiply.
#[derive(Clone, Debug)]
pub struct ConjPairPoleBankSoa {
    pole_ts_re: Vec<f32>,
    pole_ts_im: Vec<f32>,
    inv_pole_re: Vec<f32>,
    inv_pole_im: Vec<f32>,
    pole_corr_re: Vec<f32>,
    pole_corr_im: Vec<f32>,
    psi1_re: Vec<f32>,
    psi1_im: Vec<f32>,
    r_re: Vec<f32>,
    r_im: Vec<f32>,
    x_re: Vec<f32>,
    x_im: Vec<f32>,
    /// `exp(pole_ts · Δτ)` for the currently-configured uniform tick
    /// step. Initialised to identity until `set_tick_delta_tau` runs.
    /// During delay smoothing this is the per-sample running value
    /// (`alpha_cur`), linearly interpolated toward `alpha_target`.
    alpha_re: Vec<f32>,
    alpha_im: Vec<f32>,
    /// Target alpha — destination of the current ramp.
    alpha_target_re: Vec<f32>,
    alpha_target_im: Vec<f32>,
    /// Per-sample increment: `(target - cur) · inv_interval` at the
    /// time of the last target update.
    alpha_step_re: Vec<f32>,
    alpha_step_im: Vec<f32>,
}

impl ConjPairPoleBankSoa {
    pub fn new(
        pair_poles: impl IntoIterator<Item = Complex32>,
        pair_residues: impl IntoIterator<Item = Complex32>,
        sample_rate: f32,
    ) -> Self {
        let host_ts = 1.0 / sample_rate;
        let poles: Vec<Complex32> = pair_poles.into_iter().collect();
        let residues: Vec<Complex32> = pair_residues.into_iter().collect();
        assert_eq!(poles.len(), residues.len());
        let n = poles.len();
        let mut b = Self {
            pole_ts_re: Vec::with_capacity(n),
            pole_ts_im: Vec::with_capacity(n),
            inv_pole_re: Vec::with_capacity(n),
            inv_pole_im: Vec::with_capacity(n),
            pole_corr_re: Vec::with_capacity(n),
            pole_corr_im: Vec::with_capacity(n),
            psi1_re: Vec::with_capacity(n),
            psi1_im: Vec::with_capacity(n),
            r_re: Vec::with_capacity(n),
            r_im: Vec::with_capacity(n),
            x_re: vec![0.0; n],
            x_im: vec![0.0; n],
            alpha_re: vec![1.0; n],
            alpha_im: vec![0.0; n],
            alpha_target_re: vec![1.0; n],
            alpha_target_im: vec![0.0; n],
            alpha_step_re: vec![0.0; n],
            alpha_step_im: vec![0.0; n],
        };
        for (&p, &r) in poles.iter().zip(residues.iter()) {
            let pole_ts = p * host_ts;
            let pole_corr = pole_ts.exp();
            let inv_p = p.inv();
            let psi1 =
                Complex32 { re: pole_corr.re - 1.0, im: pole_corr.im } * inv_p;
            b.pole_ts_re.push(pole_ts.re);
            b.pole_ts_im.push(pole_ts.im);
            b.inv_pole_re.push(inv_p.re);
            b.inv_pole_im.push(inv_p.im);
            b.pole_corr_re.push(pole_corr.re);
            b.pole_corr_im.push(pole_corr.im);
            b.psi1_re.push(psi1.re);
            b.psi1_im.push(psi1.im);
            b.r_re.push(r.re);
            b.r_im.push(r.im);
        }
        b
    }

    pub fn len(&self) -> usize {
        self.pole_ts_re.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pole_ts_re.is_empty()
    }

    /// Snap `alpha_cur = alpha_target = exp(pole_ts · delta_tau)`
    /// with zero step — for initial configuration or hard retargets.
    /// Costs one complex `exp` per pole.
    pub fn snap_tick_delta_tau(&mut self, delta_tau: f32) {
        for i in 0..self.len() {
            let sre = self.pole_ts_re[i] * delta_tau;
            let sim = self.pole_ts_im[i] * delta_tau;
            let m = sre.exp();
            let (s, c) = sim.sin_cos();
            let tr = m * c;
            let ti = m * s;
            self.alpha_re[i] = tr;
            self.alpha_im[i] = ti;
            self.alpha_target_re[i] = tr;
            self.alpha_target_im[i] = ti;
            self.alpha_step_re[i] = 0.0;
            self.alpha_step_im[i] = 0.0;
        }
    }

    /// Schedule a linear ramp of `alpha_cur` toward `alpha_target =
    /// exp(pole_ts · delta_tau_target)` over `1/inv_interval` samples.
    /// Costs one complex `exp` per pole — amortised across the
    /// smoothing interval.
    pub fn target_tick_delta_tau(
        &mut self,
        delta_tau_target: f32,
        inv_interval: f32,
    ) {
        for i in 0..self.len() {
            let sre = self.pole_ts_re[i] * delta_tau_target;
            let sim = self.pole_ts_im[i] * delta_tau_target;
            let m = sre.exp();
            let (s, c) = sim.sin_cos();
            let tr = m * c;
            let ti = m * s;
            self.alpha_target_re[i] = tr;
            self.alpha_target_im[i] = ti;
            self.alpha_step_re[i] = (tr - self.alpha_re[i]) * inv_interval;
            self.alpha_step_im[i] = (ti - self.alpha_im[i]) * inv_interval;
        }
    }

    /// Advance `alpha_cur` one sample toward `alpha_target`.
    pub fn advance_alpha_smoothing(&mut self) {
        for i in 0..self.len() {
            self.alpha_re[i] += self.alpha_step_re[i];
            self.alpha_im[i] += self.alpha_step_im[i];
        }
    }

    /// Snap `alpha_cur` exactly to `alpha_target` and zero the step —
    /// called at the end of a ramp to eliminate float accumulation.
    pub fn snap_alpha_to_target(&mut self) {
        self.alpha_re.copy_from_slice(&self.alpha_target_re);
        self.alpha_im.copy_from_slice(&self.alpha_target_im);
        for v in self.alpha_step_re.iter_mut() {
            *v = 0.0;
        }
        for v in self.alpha_step_im.iter_mut() {
            *v = 0.0;
        }
    }

    /// Fill caller-provided scratch with `phi[i] = exp(pole_ts[i] · tau)`.
    pub fn fill_phi(&self, tau: f32, phi_re: &mut [f32], phi_im: &mut [f32]) {
        for i in 0..self.len() {
            let sre = self.pole_ts_re[i] * tau;
            let sim = self.pole_ts_im[i] * tau;
            let m = sre.exp();
            let (s, c) = sim.sin_cos();
            phi_re[i] = m * c;
            phi_im[i] = m * s;
        }
    }

    /// In-place `phi *= alpha` per pole — incremental phasor step.
    pub fn step_phi(&self, phi_re: &mut [f32], phi_im: &mut [f32]) {
        for i in 0..self.len() {
            let pr = phi_re[i];
            let pi = phi_im[i];
            let ar = self.alpha_re[i];
            let ai = self.alpha_im[i];
            phi_re[i] = pr * ar - pi * ai;
            phi_im[i] = pr * ai + pi * ar;
        }
    }

    /// Copy the precomputed alpha table into caller scratch (useful
    /// when you need a "phi" buffer equal to alpha for a constant-Δτ
    /// advance_by call).
    pub fn copy_alpha_into(&self, phi_re: &mut [f32], phi_im: &mut [f32]) {
        phi_re[..self.len()].copy_from_slice(&self.alpha_re);
        phi_im[..self.len()].copy_from_slice(&self.alpha_im);
    }

    /// Evaluate using caller-provided phi per pole. Returns
    /// `2·Re(Σ r_k · (phi_k · x_k + psi_k · u))` with
    /// `psi_k = (phi_k - 1) · inv_pole_k`.
    pub fn evaluate_with_phi(&self, phi_re: &[f32], phi_im: &[f32], u: f32) -> f32 {
        let mut sum_re = 0.0_f32;
        for i in 0..self.len() {
            let pr = phi_re[i];
            let pi = phi_im[i];
            let mr = pr - 1.0;
            let mi = pi;
            let ipr = self.inv_pole_re[i];
            let ipi = self.inv_pole_im[i];
            let psi_re = mr * ipr - mi * ipi;
            let psi_im = mr * ipi + mi * ipr;
            let xr = self.x_re[i];
            let xi = self.x_im[i];
            let y_re = pr * xr - pi * xi + psi_re * u;
            let y_im = pr * xi + pi * xr + psi_im * u;
            sum_re += self.r_re[i] * y_re - self.r_im[i] * y_im;
        }
        2.0 * sum_re
    }

    /// Advance state by caller-provided phi per pole (state update
    /// equivalent of [`Self::evaluate_with_phi`]).
    pub fn advance_by_phi(&mut self, phi_re: &[f32], phi_im: &[f32], u: f32) {
        for i in 0..self.len() {
            let pr = phi_re[i];
            let pi = phi_im[i];
            let mr = pr - 1.0;
            let mi = pi;
            let ipr = self.inv_pole_re[i];
            let ipi = self.inv_pole_im[i];
            let psi_re = mr * ipr - mi * ipi;
            let psi_im = mr * ipi + mi * ipr;
            let xr = self.x_re[i];
            let xi = self.x_im[i];
            self.x_re[i] = patches_dsp::flush_denormal(pr * xr - pi * xi + psi_re * u);
            self.x_im[i] = patches_dsp::flush_denormal(pr * xi + pi * xr + psi_im * u);
        }
    }

    /// `advance_by` with phi computed inline from `delta_tau` — used
    /// for variable-duration segments (first/tail) where the cached
    /// alpha doesn't apply.
    pub fn advance_by(&mut self, delta_tau: f32, u: f32) {
        for i in 0..self.len() {
            let sre = self.pole_ts_re[i] * delta_tau;
            let sim = self.pole_ts_im[i] * delta_tau;
            let m = sre.exp();
            let (s, c) = sim.sin_cos();
            let pr = m * c;
            let pi = m * s;
            let mr = pr - 1.0;
            let mi = pi;
            let ipr = self.inv_pole_re[i];
            let ipi = self.inv_pole_im[i];
            let psi_re = mr * ipr - mi * ipi;
            let psi_im = mr * ipi + mi * ipr;
            let xr = self.x_re[i];
            let xi = self.x_im[i];
            self.x_re[i] = patches_dsp::flush_denormal(pr * xr - pi * xi + psi_re * u);
            self.x_im[i] = patches_dsp::flush_denormal(pr * xi + pi * xr + psi_im * u);
        }
    }

    /// Full-sample advance using cached `pole_corr`/`psi1`.
    pub fn advance(&mut self, u: f32) {
        for i in 0..self.len() {
            let pcr = self.pole_corr_re[i];
            let pci = self.pole_corr_im[i];
            let xr = self.x_re[i];
            let xi = self.x_im[i];
            self.x_re[i] = patches_dsp::flush_denormal(pcr * xr - pci * xi + self.psi1_re[i] * u);
            self.x_im[i] = patches_dsp::flush_denormal(pcr * xi + pci * xr + self.psi1_im[i] * u);
        }
    }

    pub fn reset(&mut self) {
        for v in self.x_re.iter_mut() {
            *v = 0.0;
        }
        for v in self.x_im.iter_mut() {
            *v = 0.0;
        }
    }

    /// `2·Re(Σ r_k · x_k)` — conjugate-pair doubled real output.
    pub fn real_output(&self) -> f32 {
        let mut sum_re = 0.0_f32;
        for i in 0..self.len() {
            sum_re += self.r_re[i] * self.x_re[i] - self.r_im[i] * self.x_im[i];
        }
        2.0 * sum_re
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const TS: f32 = 1.0 / SR;

    fn assert_close(got: Complex32, want: Complex32, tol: f32, ctx: &str) {
        let dr = (got.re - want.re).abs();
        let di = (got.im - want.im).abs();
        assert!(
            dr < tol && di < tol,
            "{ctx}: got ({}, {}), want ({}, {}), |d| = ({dr}, {di})",
            got.re,
            got.im,
            want.re,
            want.im
        );
    }

    // ─── Level 1: filter in isolation vs closed-form ─────────────────────────

    #[test]
    fn impulse_at_sample_zero_matches_closed_form() {
        // u[0] = 1, u[n] = 0 for n > 0.
        // At sample 0 with u=1: y(τ·Ts) = ψ(τ) = (exp(p·τ·Ts) - 1) / p
        let p = Complex32::new(-10_000.0, 3_000.0);
        let mut f = ContinuousPole::new(p, SR);
        for tau in [0.0_f32, 0.25, 0.5, 0.75, 0.99] {
            let got = f.evaluate(tau, 1.0);
            let phi = (p * (tau * TS)).exp();
            let want = (phi - Complex32::new(1.0, 0.0)) / p;
            assert_close(got, want, 1.0e-6, &format!("sample 0 τ={tau}"));
        }
        f.advance(1.0);
        // After advance, state x[1] = ψ(1). Subsequent samples with
        // u=0: y((n+τ)·Ts) = φ(τ) · pole_corr^(n-1) · ψ(1).
        for n in 1..5 {
            for tau in [0.0_f32, 0.5, 0.99] {
                let got = f.evaluate(tau, 0.0);
                let phi = (p * (tau * TS)).exp();
                let want =
                    phi * f.pole_corr.powf((n - 1) as f32) * f.psi1;
                assert_close(got, want, 1.0e-5, &format!("sample {n} τ={tau}"));
            }
            f.advance(0.0);
        }
    }

    #[test]
    fn dc_steady_state_matches_theory() {
        // For sustained u = U, steady state satisfies 0 = p·x + U,
        // so x_ss = -U/p. Input residue is 1/(s-p), DC gain = -1/p.
        let p = Complex32::new(-1_000.0, 500.0);
        let mut f = ContinuousPole::new(p, SR);
        let u = 1.0_f32;
        // Decay time: 1/Re(p) = 1 ms; 100 ms is 100τ, well settled.
        for _ in 0..((SR * 0.1) as usize) {
            f.advance(u);
        }
        let want = -Complex32::new(u, 0.0) / p;
        assert_close(f.state(), want, 1.0e-4, "DC steady state");
    }

    // ─── Level 2: stitch consistency ─────────────────────────────────────────

    #[test]
    fn evaluate_tau_one_equals_next_sample_tau_zero() {
        // Continuous-time output is continuous across held-input
        // boundaries even when u changes. Verify:
        //   evaluate(1, u[n])  ==  advance(u[n]); evaluate(0, u[n+1])
        let p = Complex32::new(-10_000.0, 7_500.0);
        let mut f = ContinuousPole::new(p, SR);
        let inputs = [1.0_f32, 0.3, -0.7, 0.0, 0.5, -0.2];
        for pair in inputs.windows(2) {
            let u_n = pair[0];
            let u_np1 = pair[1];
            let at_end = f.evaluate(1.0, u_n);
            f.advance(u_n);
            let at_start = f.evaluate(0.0, u_np1);
            assert_close(at_end, at_start, 1.0e-6, "stitch");
        }
    }

    #[test]
    fn advance_equals_evaluate_at_tau_one() {
        let p = Complex32::new(-5_000.0, 20_000.0);
        let mut f = ContinuousPole::new(p, SR);
        let u = 0.7_f32;
        let want = f.evaluate(1.0, u);
        f.advance(u);
        assert_close(f.state(), want, 1.0e-6, "advance == evaluate(1)");
    }

    #[test]
    fn sub_sample_evaluation_is_monotonic_decay_in_magnitude() {
        // After a single impulse, the filter's magnitude decays like
        // |exp(Re(p)·t)|. Check monotonicity across sub-samples.
        let p = Complex32::new(-20_000.0, 1_000.0);
        let mut f = ContinuousPole::new(p, SR);
        f.advance(1.0);
        f.advance(0.0);
        // Now evaluate a dense grid within sample n=1 (u=0) and
        // assert the magnitude is non-increasing.
        let mut prev_mag = f32::INFINITY;
        for i in 0..20 {
            let tau = i as f32 / 19.0;
            let val = f.evaluate(tau, 0.0);
            let mag = (val.re * val.re + val.im * val.im).sqrt();
            assert!(
                mag <= prev_mag * 1.000_001,
                "magnitude should not grow: τ={tau} mag={mag} prev={prev_mag}"
            );
            prev_mag = mag;
        }
    }

    // ─── Level 3: cross-check against host-rate IIR ──────────────────────────

    #[test]
    fn host_rate_samples_match_impulse_invariant_iir() {
        // `evaluate(0)` at each sample should trace the exact IIR
        // recurrence `y[n+1] = pole_corr · y[n] + ψ1 · u[n]`. That's
        // the discrete state advance, so it's also a consistency
        // check against `advance`.
        let p = Complex32::new(-12_000.0, 8_000.0);
        let mut f = ContinuousPole::new(p, SR);
        let mut y = Complex32::new(0.0, 0.0);
        let inputs: Vec<f32> =
            (0..200).map(|i| (i as f32 * 0.03).sin()).collect();
        for &u in &inputs {
            let at_start = f.evaluate(0.0, u);
            assert_close(at_start, y, 1.0e-5, "start of sample == state");
            y = f.pole_corr * y + f.psi1 * Complex32::new(u, 0.0);
            f.advance(u);
        }
    }

    // ─── Bank tests ──────────────────────────────────────────────────────────

    #[test]
    fn conjugate_pole_pair_gives_real_output() {
        // A pole `p` with residue `r` and its conjugate `p*` with
        // residue `r*` produces a purely real response for real input.
        let p = Complex32::new(-5_000.0, 12_000.0);
        let r = Complex32::new(1.0, 2.0);
        let bank = ContinuousPoleBank::new(
            [p, p.conj()],
            [r, r.conj()],
            SR,
        );
        // Real output only makes sense via the bank's evaluate, which
        // takes Re(sum). Check that summing without Re would still be
        // real anyway — i.e. the imag part cancels.
        let mut bank2 = bank.clone();
        bank2.advance(1.0);
        bank2.advance(0.0);
        let got = bank2.evaluate(0.5, 0.0);
        // Manually sum full complex to verify imag ≈ 0.
        let mut sum = Complex32::new(0.0, 0.0);
        let pc = ContinuousPole::new(p, SR);
        let pc_conj = ContinuousPole::new(p.conj(), SR);
        // Re-run scalar pair to mirror bank2's state.
        let (mut x0, mut x1) = (Complex32::new(0.0, 0.0), Complex32::new(0.0, 0.0));
        for &u in &[1.0_f32, 0.0] {
            x0 = pc.pole_corr * x0 + pc.psi1 * Complex32::new(u, 0.0);
            x1 = pc_conj.pole_corr * x1 + pc_conj.psi1 * Complex32::new(u, 0.0);
        }
        let phi0 = (pc.pole() * (0.5 * TS)).exp();
        let phi1 = (pc_conj.pole() * (0.5 * TS)).exp();
        sum += r * (phi0 * x0);
        sum += r.conj() * (phi1 * x1);
        assert!(
            sum.im.abs() < 1.0e-4,
            "conjugate pair should cancel imag, got {}",
            sum.im
        );
        // Bank's `evaluate` drops the imag part; check real agrees.
        assert!((got - sum.re).abs() < 1.0e-4, "bank.re {got} vs hand-summed {}", sum.re);
    }
}
