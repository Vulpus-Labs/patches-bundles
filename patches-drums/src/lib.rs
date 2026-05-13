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
pub mod snare;
pub mod clap_drum;
pub mod hihat;
pub mod tom;
pub mod claves;
pub mod cymbal;

#[cfg(test)]
mod test_support;

pub use kick::Kick;
pub use snare::Snare;
pub use clap_drum::ClapDrum;
pub use hihat::{ClosedHiHat, OpenHiHat};
pub use tom::Tom;
pub use claves::Claves;
pub use cymbal::Cymbal;

/// Register every module in this crate with the supplied registry.
///
/// Transition shim mirroring `patches_vintage::register`: in-process
/// consumers (`patches_modules::default_registry` during E146 phase A)
/// use this until 0876 lands the `PluginScanner` stdlib path and the
/// drum cdylib is loaded the same way third-party bundles are.
pub fn register(r: &mut patches_sdk::registry::Registry) {
    r.register::<Kick>();
    r.register::<Snare>();
    r.register::<ClapDrum>();
    r.register::<ClosedHiHat>();
    r.register::<OpenHiHat>();
    r.register::<Tom>();
    r.register::<Claves>();
    r.register::<Cymbal>();
}

// ── FFI bundle export ────────────────────────────────────────────────────────
//
// Mirrors the vintage bundle: one `export_modules!` invocation emits
// the per-module ABI entry points, a combined vtable array, and the
// `patches_plugin_init` symbol that `PluginScanner` reads.
patches_sdk::export_modules! {
    (ffi_kick,         Kick,         "Kick",         1),
    (ffi_snare,        Snare,        "Snare",        1),
    (ffi_clap_drum,    ClapDrum,     "Clap",         1),
    (ffi_closed_hihat, ClosedHiHat,  "ClosedHiHat",  1),
    (ffi_open_hihat,   OpenHiHat,    "OpenHiHat",    1),
    (ffi_tom,          Tom,          "Tom",          1),
    (ffi_claves,       Claves,       "Claves",       1),
    (ffi_cymbal,       Cymbal,       "Cymbal",       1),
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
        "Snare",
        "Clap",
        "ClosedHiHat",
        "OpenHiHat",
        "Tom",
        "Claves",
        "Cymbal",
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
