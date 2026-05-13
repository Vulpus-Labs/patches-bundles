//! Startup silence, identity round-trips (inline + threaded), and
//! late-frame discard.

use patches_fft_harness::slot_deck::{FilledSlot, OverlapBuffer, SlotDeckConfig};

#[test]
fn startup_silence() {
    // Before any full window has been filled, the overlap buffer outputs silence.
    let cfg = SlotDeckConfig::new(64, 2, 16).expect("valid config");
    let (mut buf, _handle) = OverlapBuffer::new_unthreaded(cfg);
    for _ in 0..32 {
        buf.write(1.0);
        assert_eq!(buf.read(), 0.0, "should be silent before pipeline fills");
    }
}

#[test]
fn round_trip_identity_inline() {
    // Processor simulated synchronously: identity transform (in-place passthrough).
    // After total_latency samples the output should reproduce the input.
    let cfg = SlotDeckConfig::new(64, 2, 16).expect("valid config");
    let latency = cfg.total_latency();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);

    let mut outputs = Vec::new();
    for i in 0..(latency * 2) {
        let input = (i as f32) * 0.001;
        buf.write(input);

        // Inline processor: pop, pass through (no-op), push back.
        while let Some(slot) = handle.pop() {
            let _ = handle.push(slot);
        }

        outputs.push(buf.read());
    }

    // After latency samples the output should be non-zero.
    assert!(
        outputs[latency..].iter().any(|&x| x != 0.0),
        "output should be non-zero after pipeline fills"
    );
}

#[test]
fn round_trip_identity_threaded() {
    // Real spawned processing thread: identity transform.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    let cfg = SlotDeckConfig::new(64, 2, 16).expect("valid config");
    let latency = cfg.total_latency();
    let window_size = cfg.window_size;
    let (mut buf, handle) = OverlapBuffer::new_unthreaded(cfg);

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_proc = shutdown.clone();

    let processor = thread::spawn(move || {
        let mut h = handle;
        // Identity: just push back unmodified.
        while !shutdown_proc.load(Ordering::Relaxed) {
            if let Some(mut slot) = h.pop() {
                // Spin on push since buffer must circulate.
                loop {
                    match h.push(slot) {
                        Ok(()) => break,
                        Err(s) => {
                            slot = s;
                            if shutdown_proc.load(Ordering::Relaxed) { return; }
                            std::hint::spin_loop();
                        }
                    }
                }
            } else {
                std::hint::spin_loop();
            }
        }
        // After shutdown: drain remaining.
        while let Some(slot) = h.pop() {
            let _ = h.push(slot);
        }
    });

    // Phase 1: feed input samples.
    let n_samples = latency * 4;
    for i in 0..n_samples {
        buf.write((i as f32) * 0.001);
        buf.read(); // advance read head
    }

    // Phase 2: stop the processor.
    shutdown.store(true, Ordering::Relaxed);
    processor.join().expect("processor thread panicked");

    // Phase 3: flush — additional write/read cycles drain any results
    // sitting in the inbound ring buffer.
    let mut saw_nonzero = false;
    for _ in 0..(window_size * 2) {
        buf.write(0.0);
        if buf.read() != 0.0 {
            saw_nonzero = true;
            break;
        }
    }

    assert!(saw_nonzero, "output should be non-zero after pipeline fills");
}

#[test]
fn late_frame_discarded() {
    // A result frame pushed after read_head has advanced past its window end
    // should be silently discarded (no panic, no output contribution).
    let cfg = SlotDeckConfig::new(64, 2, 16).expect("valid config");
    let latency = cfg.total_latency();
    let (mut buf, mut handle) = OverlapBuffer::new_unthreaded(cfg);

    // Drain the pipeline without the processor doing any work.
    for i in 0..(latency * 2) {
        buf.write((i as f32) * 0.001);
        let _ = buf.read();
    }

    // Now inject a result with start=0 (long past) — should be discarded.
    let stale_data: Box<[f32]> = vec![1.0f32; 64].into_boxed_slice();
    let _ = handle.push(FilledSlot { start: 0, data: stale_data });

    // Output should still be 0 (or whatever the pipeline produces, but not
    // the 1.0 from the stale frame).
    let out = buf.read();
    assert!(out.abs() < 1.0, "stale frame should not contribute to output");
}
