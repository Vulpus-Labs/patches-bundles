//! Pool exhaustion recovery and edge cases: starvation recovery, slow
//! processor, return-channel full, buffer recycling.

use patches_fft_harness::slot_deck::{OverlapBuffer, SlotDeckConfig};

#[test]
fn pool_starvation_recovery() {
    // After all buffers are consumed (no recycling), returning buffers should
    // restore normal operation — no stuck state.
    let cfg = SlotDeckConfig::new(32, 2, 8).expect("valid config");
    let latency = cfg.total_latency();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);

    // Phase 1: Write samples but don't process — starve the pool.
    let starvation_samples = latency * 2;
    for i in 0..starvation_samples {
        buf.write((i as f32) * 0.001);
        let _ = buf.read(); // output will be silence
    }

    // Phase 2: Now start processing — pop all available, push back.
    let mut recovered_nonzero = false;
    let recovery_samples = latency * 4;
    for i in 0..recovery_samples {
        buf.write(1.0);

        // Process all available frames (identity)
        while let Some(slot) = handle.pop() {
            let _ = handle.push(slot);
        }

        let v = buf.read();
        if v.abs() > 0.0 && i > latency {
            recovered_nonzero = true;
        }
    }

    assert!(
        recovered_nonzero,
        "output should become non-zero after recovery from pool starvation"
    );
}

#[test]
fn slow_processor_degrades_gracefully() {
    // A processor that only processes every Nth frame should produce silence
    // for missed frames, not corruption.
    let cfg = SlotDeckConfig::new(32, 2, 8).expect("valid config");
    let latency = cfg.total_latency();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);

    let mut frame_count = 0usize;
    let n_samples = latency * 4;
    let mut all_finite = true;

    for _ in 0..n_samples {
        buf.write(1.0);

        // Only process every 3rd frame — simulate slow processor
        while let Some(slot) = handle.pop() {
            frame_count += 1;
            if frame_count.is_multiple_of(3) {
                let _ = handle.push(slot);
            }
            // Frames not pushed back are dropped — the buffer is lost.
            // This tests that the audio thread degrades gracefully.
        }

        let v = buf.read();
        if !v.is_finite() {
            all_finite = false;
        }
    }

    assert!(all_finite, "all output samples should be finite even with slow processor");
}

#[test]
fn return_channel_full_does_not_panic() {
    // If the inbound ring buffer is full, push should return Err, not panic.
    let cfg = SlotDeckConfig::new(32, 2, 8).expect("valid config");
    let pool_size = cfg.pool_size();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);

    // Fill the inbound channel by pushing many result frames without reading.
    for _ in 0..pool_size * 2 {
        buf.write(1.0);

        while let Some(slot) = handle.pop() {
            // Push result — may fail if channel full; that's OK.
            let _ = handle.push(slot);
        }
        // Deliberately not calling buf.read() to let results accumulate.
    }

    // Now read should drain without panic.
    for _ in 0..pool_size * 4 {
        buf.write(0.0);
        let _ = buf.read(); // must not panic
    }
}

#[test]
fn buffer_recycling_preserves_pool() {
    // After a burst of fill/process/drain cycles, the pool should recover
    // its buffers and continue producing filled slots.
    let cfg = SlotDeckConfig::new(32, 2, 8).expect("valid config");
    let latency = cfg.total_latency();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);

    // Phase 1: run a full cycle — write, process, read.
    let n_samples = latency * 3;
    for i in 0..n_samples {
        buf.write((i as f32) * 0.001);
        while let Some(slot) = handle.pop() {
            let _ = handle.push(slot);
        }
        let _ = buf.read();
    }

    // Phase 2: continue — should still produce non-zero output.
    let mut saw_nonzero = false;
    for i in 0..n_samples {
        buf.write(1.0);
        while let Some(slot) = handle.pop() {
            let _ = handle.push(slot);
        }
        let v = buf.read();
        if v != 0.0 && i > latency {
            saw_nonzero = true;
        }
    }

    assert!(
        saw_nonzero,
        "after recycling, pipeline should continue producing non-zero output"
    );
}
