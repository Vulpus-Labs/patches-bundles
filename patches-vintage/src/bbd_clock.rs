//! Standalone prototype of BBD clock timing, decoupled from the
//! filter bank and delay ring.
//!
//! A BBD chip clocks at `2 · stages / delay_seconds` Hz, alternating
//! **write** half-phases (charge sampled in) and **read** half-phases
//! (charge sampled out). The clock is asynchronous to the host sample
//! rate — at short delays it runs faster than the host, at long
//! delays slower. Per host sample, zero or more BBD half-ticks may
//! fire, each at a specific sub-sample instant.
//!
//! This module handles only the timing: given a host sample rate and
//! a BBD clock rate, `BbdClock::step` yields a sequence of
//! `(TickPhase, sub_sample_tau)` for one host sample's worth of
//! elapsed time, where `tau ∈ [0, 1)` is the fractional position
//! within the current host sample at which the tick fires. Consumers
//! can layer a source signal, filter, and bucket ring on top without
//! re-implementing the timing.

/// Write (bucket-input) or read (bucket-output) half-phase of the
/// BBD clock. Each full bucket cycle is one `Write` tick followed by
/// one `Read` tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickPhase {
    Write,
    Read,
}

/// One BBD clock half-tick firing within a host sample.
#[derive(Clone, Copy, Debug)]
pub struct Tick {
    pub phase: TickPhase,
    /// Sub-sample time at which the tick fires, in `[0, 1)`.
    pub tau: f32,
    /// Monotonic index across all ticks ever fired.
    pub index: u64,
}

/// BBD clock generator. Owns the sub-sample time carry and phase
/// alternation; free of filter/delay state.
pub struct BbdClock {
    host_ts: f32,
    /// `1 / host_ts` — precomputed for sub-sample τ without a divide.
    inv_host_ts: f32,
    bbd_ts: f32,
    /// Sub-sample carry in seconds, in `[0, host_ts)`.
    tn: f32,
    even_on: bool,
    tick_index: u64,
}

impl BbdClock {
    pub fn new(host_sample_rate: f32) -> Self {
        Self {
            host_ts: 1.0 / host_sample_rate,
            inv_host_ts: host_sample_rate,
            bbd_ts: 0.0,
            tn: 0.0,
            even_on: true,
            tick_index: 0,
        }
    }

    /// Set the BBD half-clock period directly. The **full** bucket
    /// cycle is `2 · bbd_ts` seconds.
    pub fn set_bbd_ts(&mut self, bbd_ts_seconds: f32) {
        // Floor is `host_ts * 0.01` — protects the `while tn < host_ts`
        // loop from firing thousands of ticks per host sample at a
        // degenerate clock rate.
        self.bbd_ts = bbd_ts_seconds.max(self.host_ts * 0.01);
    }

    /// Convenience: set clock from a delay time and stage count.
    /// `clock_rate = 2·stages/delay`, `bbd_ts = 1/clock_rate`.
    pub fn set_delay(&mut self, delay_seconds: f32, stages: usize) {
        // bbd_ts = 1 / clock_rate = delay / (2·stages); collapses two
        // divides into one.
        let bbd_ts = delay_seconds.max(1.0e-5) / (2.0 * stages as f32);
        self.set_bbd_ts(bbd_ts);
    }

    pub fn bbd_ts(&self) -> f32 {
        self.bbd_ts
    }

    pub fn host_ts(&self) -> f32 {
        self.host_ts
    }

    pub fn reset(&mut self) {
        self.tn = 0.0;
        self.even_on = true;
        self.tick_index = 0;
    }

    /// Advance one host sample; for each BBD tick that fires within
    /// this sample, call `on_tick`. Sub-sample time `tau` is relative
    /// to the start of the current host sample and lies in `[0, 1)`.
    pub fn step<F: FnMut(Tick)>(&mut self, mut on_tick: F) {
        // Largest f32 strictly below 1.0 (= `1.0f32.next_down()`).
        // `tn · inv_host_ts` can round up to exactly 1.0 even when
        // `tn < host_ts`; pin τ below 1 so `Tick::tau ∈ [0, 1)` holds.
        const MAX_TAU: f32 = f32::from_bits(0x3F7F_FFFF);
        while self.tn < self.host_ts {
            let tau = (self.tn * self.inv_host_ts).min(MAX_TAU);
            let tick = Tick {
                phase: if self.even_on { TickPhase::Write } else { TickPhase::Read },
                tau,
                index: self.tick_index,
            };
            self.tick_index += 1;
            on_tick(tick);
            self.even_on = !self.even_on;
            self.tn += self.bbd_ts;
        }
        self.tn -= self.host_ts;
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn run_and_count(clock: &mut BbdClock, host_samples: usize) -> (usize, usize) {
        let mut writes = 0;
        let mut reads = 0;
        for _ in 0..host_samples {
            clock.step(|tick| match tick.phase {
                TickPhase::Write => writes += 1,
                TickPhase::Read => reads += 1,
            });
        }
        (writes, reads)
    }

    #[test]
    fn tick_density_matches_clock_ratio_at_short_delay() {
        // 3 ms delay, 256 stages → BBD clock ≈ 170.67 kHz.
        // Half-tick rate = 2 · clock = 341.33 kHz? No — clock_rate =
        // 2·stages/delay already counts both half-phases per cycle,
        // so half-ticks fire at clock_rate = 170.67 kHz, one every
        // bbd_ts = 5.86 μs.
        let mut c = BbdClock::new(SR);
        c.set_delay(0.003, 256);
        let host_samples = 10_000;
        let (w, r) = run_and_count(&mut c, host_samples);
        let total = (w + r) as f32;
        let expected = host_samples as f32 * c.host_ts() / c.bbd_ts();
        let err = (total - expected).abs() / expected;
        assert!(err < 1e-3, "tick count {total} vs expected {expected} (err {err})");
    }

    #[test]
    fn tick_density_matches_clock_ratio_at_long_delay() {
        // 80 ms delay, 1024 stages → clock ≈ 25.6 kHz, below host rate.
        // Expect < 1 tick per host sample on average.
        let mut c = BbdClock::new(SR);
        c.set_delay(0.080, 1024);
        let host_samples = 10_000;
        let (w, r) = run_and_count(&mut c, host_samples);
        let total = (w + r) as f32;
        let expected = host_samples as f32 * c.host_ts() / c.bbd_ts();
        let err = (total - expected).abs() / expected;
        assert!(err < 1e-3, "tick count {total} vs expected {expected}");
        // Sanity: fewer ticks than host samples.
        assert!((w + r) < host_samples);
    }

    #[test]
    fn write_and_read_phases_alternate() {
        let mut c = BbdClock::new(SR);
        c.set_delay(0.003, 256);
        let mut phases: Vec<TickPhase> = Vec::new();
        for _ in 0..100 {
            c.step(|t| phases.push(t.phase));
        }
        for pair in phases.windows(2) {
            assert_ne!(pair[0], pair[1], "phases must alternate");
        }
        assert!(matches!(phases.first(), Some(TickPhase::Write)));
    }

    #[test]
    fn tau_always_in_unit_interval() {
        let mut c = BbdClock::new(SR);
        c.set_delay(0.0175, 512); // arbitrary non-aligned delay
        for _ in 0..1000 {
            c.step(|t| {
                assert!(
                    t.tau >= 0.0 && t.tau < 1.0,
                    "tau out of [0,1): {}",
                    t.tau
                );
            });
        }
    }

    #[test]
    fn tick_indices_are_monotonic() {
        let mut c = BbdClock::new(SR);
        c.set_delay(0.005, 256);
        let mut last: Option<u64> = None;
        for _ in 0..500 {
            c.step(|t| {
                if let Some(prev) = last {
                    assert_eq!(t.index, prev + 1, "index gap");
                }
                last = Some(t.index);
            });
        }
    }

    #[test]
    fn clock_change_midstream_preserves_density() {
        // Half-speed run: 100 host samples at one rate, then 100 at
        // another. Total tick count should equal the sum of each
        // segment's prediction.
        let mut c = BbdClock::new(SR);
        c.set_delay(0.003, 256);
        let bbd_ts_a = c.bbd_ts();
        let (w1, r1) = run_and_count(&mut c, 100);
        c.set_delay(0.010, 256);
        let bbd_ts_b = c.bbd_ts();
        let (w2, r2) = run_and_count(&mut c, 100);
        let total = (w1 + r1 + w2 + r2) as f32;
        let expected = 100.0 * c.host_ts() / bbd_ts_a + 100.0 * c.host_ts() / bbd_ts_b;
        let err = (total - expected).abs() / expected;
        assert!(err < 0.01, "total {total} vs expected {expected} (err {err})");
    }

    #[test]
    fn zoh_round_trip_preserves_low_frequency_content() {
        // Drive the clock with a sine evaluated at host sample times;
        // sample-and-hold the most recent host-sample value at each
        // BBD tick; reconstruct a host-rate stream by reading the
        // most-recent-tick value at each host boundary. Low-frequency
        // content should survive within reasonable tolerance —
        // validates the timing machinery end-to-end without any
        // filtering or delay.
        let mut c = BbdClock::new(SR);
        c.set_delay(0.003, 256); // fast clock, ~3.5 ticks per host sample
        let freq = 440.0_f32;
        let amp = 0.5_f32;
        let n = (SR * 0.1) as usize;
        let mut held_host;
        let mut held_tick = 0.0_f32;
        let mut peak_in = 0.0_f32;
        let mut peak_out = 0.0_f32;
        let settle = (SR * 0.01) as usize;
        for i in 0..n {
            let t = i as f32 / SR;
            let x = amp * (std::f32::consts::TAU * freq * t).sin();
            held_host = x;
            c.step(|_tick| {
                // Pretend the BBD samples the current host-held value.
                held_tick = held_host;
            });
            if i > settle {
                peak_in = peak_in.max(x.abs());
                peak_out = peak_out.max(held_tick.abs());
            }
        }
        let gain = peak_out / peak_in;
        assert!(
            gain > 0.9 && gain < 1.1,
            "ZOH round-trip gain {gain} (in {peak_in}, out {peak_out})"
        );
    }
}
