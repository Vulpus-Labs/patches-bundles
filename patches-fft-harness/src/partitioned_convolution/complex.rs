/// Complex multiply-accumulate in CMSIS packed format.
///
/// For each frequency bin, computes `acc[k] += a[k] * b[k]` where multiplication
/// is complex. DC and Nyquist bins (indices 0 and 1) are real-only.
///
/// Uses chunked iteration (4 floats = 2 complex numbers per step) to give
/// LLVM's auto-vectorizer a clean loop body to work with.
///
/// # Panics
///
/// Panics if `acc`, `a`, and `b` do not all have the same length, or if
/// the length is less than 4 or not a multiple of 2.
pub fn complex_multiply_accumulate_packed(acc: &mut [f32], a: &[f32], b: &[f32]) {
    let n = acc.len();
    assert_eq!(n, a.len());
    assert_eq!(n, b.len());
    assert!(n >= 4 && n.is_multiple_of(2));

    // DC (real-only)
    acc[0] += a[0] * b[0];
    // Nyquist (real-only)
    acc[1] += a[1] * b[1];

    // Interior complex bins in chunks of 2 complex numbers (4 floats).
    // Processing pairs lets LLVM emit wider vector ops on ARM NEON / x86 SSE.
    let interior_acc = &mut acc[2..];
    let interior_a = &a[2..];
    let interior_b = &b[2..];

    let chunks_acc = interior_acc.chunks_exact_mut(4);
    let chunks_a = interior_a.chunks_exact(4);
    let chunks_b = interior_b.chunks_exact(4);

    let remainder_start = 2 + chunks_acc.len() * 4;

    for ((c_acc, c_a), c_b) in chunks_acc.zip(chunks_a).zip(chunks_b) {
        let ar0 = c_a[0];
        let ai0 = c_a[1];
        let ar1 = c_a[2];
        let ai1 = c_a[3];
        let br0 = c_b[0];
        let bi0 = c_b[1];
        let br1 = c_b[2];
        let bi1 = c_b[3];
        c_acc[0] += ar0 * br0 - ai0 * bi0;
        c_acc[1] += ar0 * bi0 + ai0 * br0;
        c_acc[2] += ar1 * br1 - ai1 * bi1;
        c_acc[3] += ar1 * bi1 + ai1 * br1;
    }

    // Handle a trailing odd complex number if the interior bin count is odd.
    if remainder_start < n {
        let ar = a[remainder_start];
        let ai = a[remainder_start + 1];
        let br = b[remainder_start];
        let bi = b[remainder_start + 1];
        acc[remainder_start] += ar * br - ai * bi;
        acc[remainder_start + 1] += ar * bi + ai * br;
    }
}

/// Complex multiply in CMSIS packed format (non-accumulating).
///
/// Computes `out[k] = a[k] * b[k]` for each frequency bin.
pub fn complex_multiply_packed(out: &mut [f32], a: &[f32], b: &[f32]) {
    let n = out.len();
    assert_eq!(n, a.len());
    assert_eq!(n, b.len());
    assert!(n >= 4 && n.is_multiple_of(2));

    out[0] = a[0] * b[0];
    out[1] = a[1] * b[1];

    let interior_out = &mut out[2..];
    let interior_a = &a[2..];
    let interior_b = &b[2..];

    let chunks_out = interior_out.chunks_exact_mut(4);
    let chunks_a = interior_a.chunks_exact(4);
    let chunks_b = interior_b.chunks_exact(4);

    let remainder_start = 2 + chunks_out.len() * 4;

    for ((c_out, c_a), c_b) in chunks_out.zip(chunks_a).zip(chunks_b) {
        let ar0 = c_a[0];
        let ai0 = c_a[1];
        let ar1 = c_a[2];
        let ai1 = c_a[3];
        let br0 = c_b[0];
        let bi0 = c_b[1];
        let br1 = c_b[2];
        let bi1 = c_b[3];
        c_out[0] = ar0 * br0 - ai0 * bi0;
        c_out[1] = ar0 * bi0 + ai0 * br0;
        c_out[2] = ar1 * br1 - ai1 * bi1;
        c_out[3] = ar1 * bi1 + ai1 * br1;
    }

    if remainder_start < n {
        let ar = a[remainder_start];
        let ai = a[remainder_start + 1];
        let br = b[remainder_start];
        let bi = b[remainder_start + 1];
        out[remainder_start] = ar * br - ai * bi;
        out[remainder_start + 1] = ar * bi + ai * br;
    }
}
