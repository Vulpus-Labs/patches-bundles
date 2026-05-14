# patches-bundles

Stdlib module bundles for the [Patches](https://github.com/Vulpus-Labs/patches)
modular-audio framework. Each bundle is a `cdylib + rlib` package
loaded into the host at runtime via `PluginScanner`.

## Bundles

| Crate | Modules |
|-------|---------|
| [`patches-vintage`](patches-vintage/) | VChorus, VBbd, VStereoBbd, VDco, VPolyDco, VFlanger, VFlangerStereo, VReverb, VLadder, VPolyLadder, VOtaVcf, VOtaPolyVcf |
| [`patches-drums`](patches-drums/) | Kick, Snare, ClapDrum, OpenHiHat, ClosedHiHat, Tom, Cymbal, Claves |
| [`patches-fft-harness`](patches-fft-harness/) | Reusable FFT DSP primitives (rlib only) |
| [`patches-fft-bundle`](patches-fft-bundle/) | ConvolutionReverb, StereoConvolutionReverb, PitchShift |

## Layout

```text
patches-bundles/
├── patches-vintage/      cdylib + rlib
├── patches-drums/        cdylib + rlib
├── patches-fft-harness/  rlib (shared by fft-bundle)
└── patches-fft-bundle/   cdylib + rlib
```

All four crates share a single workspace (`Cargo.toml`), a single
license (MIT), a single release tag, and a single CI pipeline.

## Status

Extracted from the main `patches` monorepo under
[E146](https://github.com/Vulpus-Labs/patches/blob/main/epics/closed/E146-monorepo-split.md)
(tickets 0882 / 0883 / 0884) and subsequently consolidated into this
single repo.

`patches-dsp` and `patches-io` are pinned to the main repo's
`v0.7.2` tag. `patches-io` has no crates.io presence and stays
git-only.

## Build

```sh
cargo build           # all four crates
cargo test            # tests across the workspace
cargo clippy          # lint
```

The cdylib artefacts land in `target/{debug,release}/`. To install
them into a host's plugin search path, copy
`libpatches_vintage.{dylib,so,dll}`,
`libpatches_drums.{dylib,so,dll}`, and
`libpatches_fft_bundle.{dylib,so,dll}` into the host's
`$EXE_DIR/plugins/` directory (or set `PATCHES_PLUGIN_PATH` to point
at this workspace's `target/release/`).
