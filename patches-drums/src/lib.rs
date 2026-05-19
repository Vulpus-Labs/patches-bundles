//! `patches-drums` — analog drum synthesis bundle (kick, snare, hihat,
//! cymbal, tom, clap, claves) plus the small set of primitives they
//! share (decay envelope, pitch sweep, metallic tone, burst generator,
//! saturate).
//!
//! Built as `cdylib + rlib`: hosts load the cdylib through
//! `PluginScanner`; in-process consumers can still call [`register`]
//! to drop the seven module types into a `Registry` during the E146
//! transition.
//!
//! A later ticket (0883 under E146) cuts this crate to its own repo
//! and bumps `patches-sdk` once the FFI ABI is published.

pub mod primitives;

pub mod kick;
pub mod kick2;
pub mod snare;
pub mod clap_drum;
pub mod hihat;
pub mod hihat2;
pub mod xor_hihat;
pub mod tom;
pub mod tom2;
pub mod claves;
pub mod claves2;
pub mod cymbal;
pub mod cymbal2;
pub mod xor_cymbal;

#[cfg(test)]
mod test_support;

pub use kick::Kick;
pub use kick2::Kick2;
pub use snare::Snare;
pub use clap_drum::ClapDrum;
pub use hihat::{ClosedHiHat, OpenHiHat};
pub use hihat2::{ClosedHiHat2, OpenHiHat2};
pub use xor_hihat::{XorClosedHiHat, XorOpenHiHat};
pub use tom::Tom;
pub use tom2::Tom2;
pub use claves::Claves;
pub use claves2::Claves2;
pub use cymbal::Cymbal;
pub use cymbal2::Cymbal2;
pub use xor_cymbal::XorCymbal;

/// Register every module in this crate with the supplied registry.
///
/// Transition shim mirroring `patches_vintage::register`: in-process
/// consumers (`patches_modules::default_registry` during E146 phase A)
/// use this until 0876 lands the `PluginScanner` stdlib path and the
/// drum cdylib is loaded the same way third-party bundles are.
pub fn register(r: &mut patches_sdk::registry::Registry) {
    r.register::<Kick>();
    r.register::<Kick2>();
    r.register::<Snare>();
    r.register::<ClapDrum>();
    r.register::<ClosedHiHat>();
    r.register::<OpenHiHat>();
    r.register::<ClosedHiHat2>();
    r.register::<OpenHiHat2>();
    r.register::<Tom>();
    r.register::<Tom2>();
    r.register::<Claves>();
    r.register::<Claves2>();
    r.register::<Cymbal>();
    r.register::<Cymbal2>();
    r.register::<XorCymbal>();
    r.register::<XorClosedHiHat>();
    r.register::<XorOpenHiHat>();
}

// ── FFI bundle export ────────────────────────────────────────────────────────
//
// Mirrors the vintage bundle: one `export_modules!` invocation emits
// the per-module ABI entry points, a combined vtable array, and the
// `patches_plugin_init` symbol that `PluginScanner` reads.
patches_sdk::export_modules! {
    (ffi_kick,         Kick,         "Kick",         1),
    (ffi_kick2,        Kick2,        "Kick2",        1),
    (ffi_snare,        Snare,        "Snare",        1),
    (ffi_clap_drum,    ClapDrum,     "Clap",         1),
    (ffi_closed_hihat, ClosedHiHat,  "ClosedHiHat",  1),
    (ffi_open_hihat,   OpenHiHat,    "OpenHiHat",    1),
    (ffi_closed_hihat2, ClosedHiHat2, "ClosedHiHat2", 1),
    (ffi_open_hihat2,  OpenHiHat2,   "OpenHiHat2",   1),
    (ffi_tom,          Tom,          "Tom",          1),
    (ffi_tom2,         Tom2,         "Tom2",         1),
    (ffi_claves,       Claves,       "Claves",       1),
    (ffi_claves2,      Claves2,      "Claves2",      1),
    (ffi_cymbal,       Cymbal,       "Cymbal",       1),
    (ffi_cymbal2,      Cymbal2,      "Cymbal2",      1),
    (ffi_xor_cymbal,   XorCymbal,    "XorCymbal",    1),
    (ffi_xor_closed_hihat, XorClosedHiHat, "XorClosedHiHat", 1),
    (ffi_xor_open_hihat,   XorOpenHiHat,   "XorOpenHiHat",   1),
}

#[cfg(test)]
mod ffi_bundle_tests {
    //! Sanity-check the bundle manifest via the rlib side. A full
    //! dylib-load round-trip lives in patches-integration-tests after
    //! 0876 wires the scanner path.

    use super::*;
    use patches_sdk::types::ABI_VERSION;

    const EXPECTED_NAMES: &[&str] = &[
        "Kick",
        "Kick2",
        "Snare",
        "Clap",
        "ClosedHiHat",
        "OpenHiHat",
        "ClosedHiHat2",
        "OpenHiHat2",
        "Tom",
        "Tom2",
        "Claves",
        "Claves2",
        "Cymbal",
        "Cymbal2",
        "XorCymbal",
        "XorClosedHiHat",
        "XorOpenHiHat",
    ];

    #[test]
    fn manifest_lists_every_drum_module() {
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
