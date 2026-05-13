//! Overload and starvation: writes must drop silently rather than block or panic.

use patches_fft_harness::slot_deck::{OverlapBuffer, SlotDeckConfig};

#[test]
fn write_overload_does_not_block() {
    // If the outbound ring buffer is full the write must silently drop, not block.
    let cfg = SlotDeckConfig::new(16, 2, 8).expect("valid config");
    let pool_size = cfg.pool_size();
    let window_size = cfg.window_size;
    let (mut buf, _handle) = OverlapBuffer::new_unthreaded(cfg);

    // Write many more samples than the pool can hold — must not block or panic.
    for i in 0..(pool_size * window_size * 4) {
        buf.write((i as f32) * 0.001);
    }
}

#[test]
fn pool_starvation_degrades_gracefully() {
    // If no free buffers are available, writes are dropped silently.
    let cfg = SlotDeckConfig::new(32, 2, 8).expect("valid config");
    let pool_size = cfg.pool_size();
    let window_size = cfg.window_size;
    let (mut buf, _handle_dropped) = OverlapBuffer::new_unthreaded(cfg);

    // Write enough samples to exhaust the pool and trigger starvation.
    for i in 0..(pool_size * window_size * 2) {
        buf.write((i as f32) * 0.001); // must not panic
    }
}
