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
cargo xtask package   # release build + stage plugins as .pxm
```

The cdylib artefacts land in `target/{debug,release}/`. `cargo xtask
package` builds in release mode and copies the cdylibs into
`release/plugins/`, renamed with a `.pxm` suffix (`patches_vintage.pxm`,
`patches_drums.pxm`, `patches_fft_bundle.pxm`).

## Install paths

The host's CLAP plugin (ticket 0899, `PluginScanner::with_global_dirs`)
scans, in order:

1. `PATCHES_PLUGIN_PATH` env var entries
2. Caller-supplied paths
3. `GlobalConfig::bundle_dirs` from `settings.toml`
4. Default bundle dir (see below) — only if it already exists

The default bundle dir is `ProjectDirs::from("", "", "Patches")
.data_dir().join("bundles")` (ticket 0897):

| OS      | Path                                                       |
|---------|------------------------------------------------------------|
| macOS   | `~/Library/Application Support/Patches/bundles`            |
| Linux   | `$XDG_DATA_HOME/Patches/bundles` (else `~/.local/share/…`) |
| Windows | `%APPDATA%\Patches\data\bundles`                           |

The host never auto-creates these directories — sandbox-safety
(0897:29-30, 0899:41). Drop the `.pxm` files in yourself, or point the
host at your own dir via `PATCHES_PLUGIN_PATH` or `settings.toml`.

## Release

CI lives in [.github/workflows/release.yml](.github/workflows/release.yml).
Push a `v*` tag (or trigger `workflow_dispatch`) to fan out two jobs:

- **macOS** — builds for `x86_64-apple-darwin` and `aarch64-apple-darwin`,
  `lipo`s the cdylibs into universal `.pxm` files, and zips
  `release/plugins/` as `patches-bundles-<version>-macos-universal.zip`.
- **Windows** — runs `cargo xtask package`, then compiles
  [installer/windows/patches-bundles.iss](installer/windows/patches-bundles.iss)
  with Inno Setup into
  `patches-bundles-<version>-windows-x64.exe`.

On tag pushes the artifacts are attached to a GitHub release. The
Windows installer defaults to `%APPDATA%\Patches\data\bundles` — the
per-user data dir the host scans automatically — and exposes the
directory page so users can redirect it if they have configured a
different `bundle_dirs` entry. Runs unprivileged (no UAC). Neither
artefact is code-signed yet, so Gatekeeper / SmartScreen will warn on
first run.
