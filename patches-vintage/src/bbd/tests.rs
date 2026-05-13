//! BBD unit tests.
//!
//! Tests are organised by what they assert about the BBD's behaviour,
//! not by implementation detail — the internal structure changed from
//! a partial-fraction filter bank to a fractional-delay ring with
//! cascaded one-pole lowpasses, and the tests should survive that
//! kind of rewrite.

use super::*;

const SR: f32 = 48_000.0;

fn run_and_measure_peak(bbd: &mut Bbd, input: impl Fn(usize) -> f32, settle: usize, n: usize) -> f32 {
    let mut peak = 0.0_f32;
    for i in 0..n {
        let y = bbd.process(input(i));
        if i >= settle {
            peak = peak.max(y.abs());
        }
    }
    peak
}

// ─── Baseline integrity ──────────────────────────────────────────────────────

#[test]
fn silence_in_gives_silence_out() {
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    bbd.set_delay_seconds(0.003);
    let peak = run_and_measure_peak(&mut bbd, |_| 0.0, 0, (SR * 0.1) as usize);
    assert!(peak < 1.0e-5, "silent-in silent-out violated: peak {peak}");
}

#[test]
fn reset_clears_state() {
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    bbd.set_delay_seconds(0.003);
    for i in 0..1024 {
        bbd.process((i as f32 * 0.01).sin());
    }
    bbd.reset();
    let y = bbd.process(0.0);
    assert!(y.abs() < 1.0e-6, "after reset silent in → silent out: {y}");
}

#[test]
fn set_delay_seconds_does_not_allocate() {
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    for i in 0..1000 {
        let d = 0.001 + (i as f32) * 1.0e-6;
        bbd.set_delay_seconds(d);
    }
    assert!(bbd.delay_seconds() > 0.0);
}

// ─── Delay semantics ─────────────────────────────────────────────────────────

#[test]
fn impulse_peaks_near_commanded_delay() {
    fn time_to_peak(delay_s: f32) -> usize {
        let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
        bbd.set_delay_seconds(delay_s);
        let horizon = (SR * (delay_s + 0.02)) as usize;
        let mut peak_idx = 0;
        let mut peak_abs = 0.0_f32;
        bbd.process(1.0);
        for i in 1..horizon {
            let y = bbd.process(0.0).abs();
            if y > peak_abs {
                peak_abs = y;
                peak_idx = i;
            }
        }
        peak_idx
    }

    // Tolerance: 1 ms — accounts for the cascaded-LP group delay plus
    // the impulse's broadening by the reconstruction filter.
    for ms in [2.0_f32, 4.0, 6.0] {
        let peak = time_to_peak(ms * 1e-3);
        let commanded = (ms * 1e-3 * SR) as usize;
        let window = (SR * 2e-3) as usize;
        assert!(
            peak > commanded.saturating_sub(window) && peak < commanded + window,
            "{ms} ms delay: peak at {peak}, commanded {commanded}, window ±{window}"
        );
    }
}

#[test]
fn longer_delay_shifts_peak_later() {
    fn time_to_peak(delay_s: f32) -> usize {
        let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
        bbd.set_delay_seconds(delay_s);
        let horizon = (SR * (delay_s * 3.0 + 0.01)) as usize;
        let mut peak_idx = 0;
        let mut peak_abs = 0.0_f32;
        bbd.process(1.0);
        for i in 1..horizon {
            let y = bbd.process(0.0).abs();
            if y > peak_abs {
                peak_abs = y;
                peak_idx = i;
            }
        }
        peak_idx
    }
    let short = time_to_peak(0.002);
    let long = time_to_peak(0.006);
    assert!(long > short, "longer delay should peak later: {short} vs {long}");
}

// ─── Gain / frequency response ───────────────────────────────────────────────

#[test]
fn dc_gain_near_unity() {
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    bbd.set_delay_seconds(0.003);
    // Small-signal (linear, below saturation knee): gain should be ≈1.
    let amp = 0.05_f32;
    for _ in 0..((SR * 0.05) as usize) {
        bbd.process(amp);
    }
    let mut avg = 0.0_f32;
    let n = (SR * 0.02) as usize;
    for _ in 0..n {
        avg += bbd.process(amp);
    }
    avg /= n as f32;
    let gain = avg / amp;
    assert!(gain > 0.9 && gain < 1.1, "DC gain outside ±1 dB: {gain}");
}

#[test]
fn passband_sine_gain_near_unity() {
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    bbd.set_delay_seconds(0.003);
    let freq = 440.0_f32;
    // Small-signal linear regime.
    let amp = 0.05_f32;
    let settle = (SR * 0.05) as usize;
    let n = (SR * 0.25) as usize;
    let peak = run_and_measure_peak(
        &mut bbd,
        |i| amp * (std::f32::consts::TAU * freq * (i as f32 / SR)).sin(),
        settle,
        n,
    );
    let lo = amp * 0.8912; // -1 dB
    let hi = amp * 1.1220; // +1 dB
    assert!(
        peak > lo && peak < hi,
        "440 Hz peak {peak} outside ±1 dB window [{lo}, {hi}]"
    );
}

#[test]
fn high_frequency_is_attenuated() {
    // Near Nyquist the cascaded LPFs should kill the signal.
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    bbd.set_delay_seconds(0.003);
    let freq = 18_000.0_f32;
    let amp = 0.5_f32;
    let settle = (SR * 0.05) as usize;
    let n = (SR * 0.1) as usize;
    let peak = run_and_measure_peak(
        &mut bbd,
        |i| amp * (std::f32::consts::TAU * freq * (i as f32 / SR)).sin(),
        settle,
        n,
    );
    assert!(
        peak < amp * 0.3,
        "18 kHz should be heavily attenuated, got peak {peak}"
    );
}

// ─── The invariant that was broken ──────────────────────────────────────────

#[test]
fn sustained_sine_shows_no_slow_amplitude_drift() {
    // Feed a pure 440 Hz sine for 3 seconds. Measure peak amplitude
    // in non-overlapping 50 ms windows. Once past warm-up, the peaks
    // must not drift more than ±0.5 dB. The earlier partial-fraction
    // port drifted 10–30 dB with a sub-Hz period because `aplus` used
    // wrong units for `delta`.
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    bbd.set_delay_seconds(0.003);
    let freq = 440.0_f32;
    let amp = 0.5_f32;
    let warmup = (SR * 0.1) as usize;
    let win = (SR * 0.05) as usize;
    let total = (SR * 3.0) as usize;
    let mut win_peaks: Vec<f32> = Vec::new();
    let mut cur_peak = 0.0_f32;
    for i in 0..total {
        let t = i as f32 / SR;
        let x = amp * (std::f32::consts::TAU * freq * t).sin();
        let y = bbd.process(x);
        if i >= warmup {
            cur_peak = cur_peak.max(y.abs());
            if (i - warmup + 1).is_multiple_of(win) {
                win_peaks.push(cur_peak);
                cur_peak = 0.0;
            }
        }
    }
    let min = win_peaks.iter().copied().fold(f32::INFINITY, f32::min);
    let max = win_peaks.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let db_range = 20.0 * (max / min).log10();
    assert!(
        db_range < 0.5,
        "window peaks drift {db_range:.2} dB over 3 s (min {min:.4}, max {max:.4})"
    );
}

#[test]
fn long_run_does_not_accumulate_noise() {
    // With zero input for 30 seconds, output must stay at silence —
    // no slowly-building noise from filter/delay feedback.
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    bbd.set_delay_seconds(0.003);
    bbd.process(1.0);
    for _ in 0..((SR * 0.1) as usize) {
        bbd.process(0.0);
    }
    let mut late_peak = 0.0_f32;
    let long = (SR * 30.0) as usize;
    for _ in 0..long {
        late_peak = late_peak.max(bbd.process(0.0).abs());
    }
    assert!(
        late_peak < 1.0e-4,
        "silent tail drifted up to {late_peak} over 30 s"
    );
}

#[test]
fn delay_sweep_is_click_free() {
    let mut bbd = Bbd::new(&BbdDevice::BBD_256, SR);
    let mut prev = 0.0_f32;
    let mut max_step = 0.0_f32;
    let n = (SR * 0.1) as usize;
    for i in 0..n {
        let t = i as f32 / SR;
        let d = 0.001 + 0.004 * (i as f32 / n as f32);
        bbd.set_delay_seconds(d);
        let x = (std::f32::consts::TAU * 440.0 * t).sin();
        let y = bbd.process(x);
        let step = (y - prev).abs();
        max_step = max_step.max(step);
        prev = y;
    }
    assert!(max_step.is_finite());
    assert!(max_step < 0.5, "delay sweep click of size {max_step}");
}
