//! `patches-fft-bundle` — FFT-based stdlib modules: pitch_shift and
//! convolution_reverb (mono + stereo). Built as `cdylib + rlib`; the
//! host loads the cdylib via `PluginScanner`, while in-process
//! consumers can still call [`register`] during the E146 transition.
//!
//! Pure parameter / Module-trait wiring lives here; the OLA/WOLA
//! scaffolding (slot_deck, partitioned_convolution, spectral
//! pitch-shifter) is in `patches-fft-harness` so third-party FFT
//! bundles can depend on the harness without pulling these modules.

pub mod pitch_shift;
pub mod convolution_reverb;

pub use pitch_shift::PitchShift;
pub use convolution_reverb::{ConvolutionReverb, StereoConvReverb};

/// Register every module in this crate with the supplied registry.
///
/// Transition shim mirroring `patches_vintage::register` /
/// `patches_drums::register`: in-process consumers use this until
/// 0876 lands the `PluginScanner` stdlib path and the bundle cdylib
/// is loaded the same way third-party bundles are.
pub fn register(r: &mut patches_sdk::registry::Registry) {
    r.register::<PitchShift>();
    r.register::<ConvolutionReverb>();
    r.register::<StereoConvReverb>();
}

// ── FFI bundle export ────────────────────────────────────────────────────────
patches_sdk::export_modules! {
    (ffi_pitch_shift,       PitchShift,       "PitchShift",       1),
    (ffi_conv_reverb,       ConvolutionReverb, "ConvReverb",      1),
    (ffi_stereo_conv_reverb, StereoConvReverb, "StereoConvReverb", 1),
}

#[cfg(test)]
mod ffi_bundle_tests {
    use super::*;
    use patches_sdk::types::ABI_VERSION;

    const EXPECTED_NAMES: &[&str] = &[
        "PitchShift",
        "ConvReverb",
        "StereoConvReverb",
    ];

    #[test]
    fn manifest_lists_every_fft_module() {
        let manifest = patches_plugin_init();
        assert_eq!(manifest.abi_version, ABI_VERSION);
        assert_eq!(manifest.count, EXPECTED_NAMES.len());

        // SAFETY: pointer comes from a process-static slice produced
        // by the macro; valid for the lifetime of the program.
        let vtables =
            unsafe { std::slice::from_raw_parts(manifest.vtables, manifest.count) };

        for (vtable, expected_name) in vtables.iter().zip(EXPECTED_NAMES) {
            assert_eq!(vtable.abi_version, ABI_VERSION, "abi drift in {expected_name}");
            // SAFETY: FFI fn ptr emitted by the macro; safe to call.
            let bytes = unsafe { (vtable.module_template)() };
            let template = patches_sdk::json::deserialize_module_descriptor_template(
                unsafe { bytes.as_slice() },
            )
            .expect("module_template returned invalid JSON");
            assert_eq!(&template.name, expected_name);
            // SAFETY: free_bytes matches FfiBytes::from_vec.
            unsafe { (vtable.free_bytes)(bytes) };
        }
    }
}
