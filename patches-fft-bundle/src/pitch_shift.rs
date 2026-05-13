//! Spectral pitch shifter module.
//!
//! Uses WOLA (weighted overlap-add) with a phase-vocoder pitch shifter running
//! on a dedicated processing thread. Parameters and CV values propagate to the
//! processing thread via shared atomics -- no mutex, no allocation on the audio
//! thread.
//!
//! # Inputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `in` | mono | Audio input |
//! | `pitch` | mono | Pitch CV ([-1,1] maps to [-12,12] semitones) |
//! | `mix` | mono | Dry/wet CV (added to parameter, clamped 0--1) |
//!
//! # Outputs
//!
//! | Port | Kind | Description |
//! |------|------|-------------|
//! | `out` | mono | Audio output |
//!
//! # Parameters
//!
//! | Name | Type | Range | Default | Description |
//! |------|------|-------|---------|-------------|
//! | `semitones` | float | -24.0--24.0 | `0.0` | Base pitch shift in semitones |
//! | `mix` | float | 0.0--1.0 | `1.0` | Dry/wet mix |
//! | `formants` | bool | -- | `false` | Preserve formant envelope |
//! | `mono` | bool | -- | `false` | Mono mode (region-based shift) |

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;

use patches_sdk::module_params;
use patches_sdk::cable_pool::CablePool;
use patches_sdk::param_frame::ParamView;
use patches_sdk::{
    AudioEnvironment, InputPort, InstanceId, ModuleDescriptor, MonoInput, MonoOutput,
    OutputPort, ParameterKind,
};
use patches_sdk::modules::{CountAxis, ModuleDescriptorTemplate, ParameterTemplate, PortTemplate};
use patches_sdk::{StructuralParams, BuildError};

module_params! {
    PitchShift {
        semitones: Float,
        mix:       Float,
        formants:  Bool,
        mono:      Bool,
    }
}

use patches_dsp::AtomicF32;
use patches_dsp::fft::RealPackedFft;
use patches_fft_harness::slot_deck::{OverlapBuffer, SlotDeckConfig};
use patches_fft_harness::spectral_pitch_shift::SpectralPitchShifter;
use patches_fft_harness::WindowBuffer;

// ---------------------------------------------------------------------------
// Shared parameters (audio thread → processing thread via atomics)
// ---------------------------------------------------------------------------

struct SharedParams {
    shift_ratio: AtomicF32,
    mix: AtomicF32,
    preserve_formants: AtomicBool,
    mono: AtomicBool,
    shutdown: AtomicBool,
}

impl SharedParams {
    fn new() -> Self {
        Self {
            shift_ratio: AtomicF32::new(1.0),
            mix: AtomicF32::new(1.0),
            preserve_formants: AtomicBool::new(false),
            mono: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

// Standard quality: 1024-sample window, 4x overlap (~46ms latency at 44.1kHz).
const STD_WINDOW_SIZE: usize = 1024;
const STD_OVERLAP_FACTOR: usize = 4;
const STD_PROCESSING_BUDGET: usize = 1024;

// High quality: 2048-sample window, 8x overlap (~93ms latency at 44.1kHz).
const HQ_WINDOW_SIZE: usize = 2048;
const HQ_OVERLAP_FACTOR: usize = 8;
const HQ_PROCESSING_BUDGET: usize = 2048;

fn hann(n: f32) -> f32 {
    (std::f32::consts::PI * n).sin().powi(2)
}

// ---------------------------------------------------------------------------
// Processing thread
// ---------------------------------------------------------------------------

fn run_processor(
    mut handle: patches_fft_harness::slot_deck::ProcessorHandle,
    shared: Arc<SharedParams>,
    analysis_window: WindowBuffer,
    synthesis_window: WindowBuffer,
    fft: RealPackedFft,
    hop_size: usize,
) {
    let window_size = fft.len();
    let mut shifter = SpectralPitchShifter::new(window_size, hop_size);

    handle.run_until_shutdown(&shared.shutdown, |slot| {
        // Read current parameters from shared atomics.
        shifter.set_shift_ratio(shared.shift_ratio.load());
        shifter.set_mix(shared.mix.load());
        shifter.set_preserve_formants(shared.preserve_formants.load(Relaxed));
        shifter.set_mono(shared.mono.load(Relaxed));

        // Analysis window → FFT → pitch shift → IFFT → synthesis window,
        // all in-place on slot.data.
        analysis_window.apply(&mut slot.data);
        fft.forward(&mut slot.data);
        shifter.transform(&mut slot.data);
        fft.inverse(&mut slot.data);
        synthesis_window.apply(&mut slot.data);
    });
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

pub struct PitchShift {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,

    // Ports
    in_audio: MonoInput,
    in_pitch: MonoInput,
    in_mix: MonoInput,
    out_audio: MonoOutput,

    // WOLA buffer (audio thread side)
    overlap_buffer: OverlapBuffer,

    // Shared parameters
    shared: Arc<SharedParams>,

    // Cached parameter values (for combining with CV)
    base_semitones: f32,
    base_mix: f32,

    // Processing thread handle (joined on drop)
    processor_thread: Option<std::thread::JoinHandle<()>>,
}

// SAFETY: PitchShift is constructed on the control thread and sent once to
// the audio thread (via Module: Send), where it remains for its lifetime.
// OverlapBuffer is !Send as a lint against casual cross-thread use, but
// single ownership transfer at plan activation is safe — it is never shared.
unsafe impl Send for PitchShift {}

impl Drop for PitchShift {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Relaxed);
        if let Some(handle) = self.processor_thread.take() {
            let _ = handle.join();
        }
    }
}

impl patches_sdk::Module for PitchShift {
    fn template() -> ModuleDescriptorTemplate {
        const T: ModuleDescriptorTemplate = ModuleDescriptorTemplate {
            name: "PitchShift",
            axes: &[CountAxis::CHANNELS],
            global_inputs: &[
                PortTemplate::mono("in"),
                PortTemplate::mono("pitch"),
                PortTemplate::mono("mix"),
            ],
            per_axis_inputs: &[],
            global_outputs: &[PortTemplate::mono("out")],
            per_axis_outputs: &[],
            realtime_params: &[
                ParameterTemplate {
                    name: params::semitones.as_str(),
                    kind: ParameterKind::Float { min: -24.0, max: 24.0, default: 0.0 },
                },
                ParameterTemplate {
                    name: params::mix.as_str(),
                    kind: ParameterKind::Float { min: 0.0, max: 1.0, default: 1.0 },
                },
                ParameterTemplate {
                    name: params::formants.as_str(),
                    kind: ParameterKind::Bool { default: false },
                },
                ParameterTemplate {
                    name: params::mono.as_str(),
                    kind: ParameterKind::Bool { default: false },
                },
            ],
            structural_params: &[
                ParameterTemplate {
                    name: "high_quality",
                    kind: ParameterKind::Bool { default: false },
                },
                ParameterTemplate {
                    name: "length",
                    kind: ParameterKind::Int { min: 0, max: 4096, default: 0 },
                },
            ],
            per_axis_realtime_params: &[],
            per_axis_structural_params: &[],
        };
        T
    }

    fn prepare(
        _audio_environment: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
        structural: &StructuralParams,
    ) -> Result<Self, BuildError> { Ok({
        let high_quality = structural.get_bool("high_quality", 0).unwrap_or(false);
        let length = structural.get_int("length", 0).unwrap_or(0) as usize;
        let (window_size, overlap_factor, default_budget) = if high_quality {
            (HQ_WINDOW_SIZE, HQ_OVERLAP_FACTOR, HQ_PROCESSING_BUDGET)
        } else {
            (STD_WINDOW_SIZE, STD_OVERLAP_FACTOR, STD_PROCESSING_BUDGET)
        };
        let processing_budget = if length == 0 {
            default_budget
        } else {
            length.next_power_of_two().clamp(128, 4096)
        };
        let config =
            SlotDeckConfig::new(window_size, overlap_factor, processing_budget)
                .expect("pitch_shift: invalid SlotDeckConfig");
        let hop_size = config.hop_size();
        let analysis_window = WindowBuffer::new(window_size, hann);
        let synthesis_window = analysis_window.normalised_wola(hop_size);
        let fft = RealPackedFft::new(window_size);

        let shared = Arc::new(SharedParams::new());
        let shared_clone = Arc::clone(&shared);

        let (overlap_buffer, join_handle) = OverlapBuffer::new(config, |handle| {
            std::thread::Builder::new()
                .name("patches-pitch-shift".into())
                .spawn(move || run_processor(handle, shared_clone, analysis_window, synthesis_window, fft, hop_size))
                .expect("pitch_shift: failed to spawn processing thread")
        });

        Self {
            instance_id,
            descriptor,
            in_audio: MonoInput::default(),
            in_pitch: MonoInput::default(),
            in_mix: MonoInput::default(),
            out_audio: MonoOutput::default(),
            overlap_buffer,
            shared,
            base_semitones: 0.0,
            base_mix: 1.0,
            processor_thread: Some(join_handle),
        }
    })}

    fn update_validated_parameters(&mut self, p: &ParamView<'_>) {
        self.base_semitones = p.get(params::semitones);
        self.base_mix = p.get(params::mix);
        self.shared.preserve_formants.store(p.get(params::formants), Relaxed);
        self.shared.mono.store(p.get(params::mono), Relaxed);
        // Push current parameter values to shared atomics.
        self.update_shared_params(0.0, 0.0);
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_audio = MonoInput::from_ports(inputs, 0);
        self.in_pitch = MonoInput::from_ports(inputs, 1);
        self.in_mix = MonoInput::from_ports(inputs, 2);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let input = pool.read_mono(&self.in_audio);
        self.overlap_buffer.write(input);
        let output = self.overlap_buffer.read();
        pool.write_mono(&self.out_audio, output);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn wants_periodic(&self) -> bool { true }

    fn periodic_update(&mut self, pool: &CablePool<'_>) {
        if !self.in_pitch.is_connected() && !self.in_mix.is_connected() {
            return;
        }
        let pitch_cv = if self.in_pitch.is_connected() {
            pool.read_mono(&self.in_pitch)
        } else {
            0.0
        };
        let mix_cv = if self.in_mix.is_connected() {
            pool.read_mono(&self.in_mix)
        } else {
            0.0
        };
        self.update_shared_params(pitch_cv, mix_cv);
    }
}

impl PitchShift {
    /// Combine base parameters with CV offsets and push to shared atomics.
    fn update_shared_params(&self, pitch_cv: f32, mix_cv: f32) {
        // pitch CV: [-1, 1] scaled and clamped to [-12, 12] semitones.
        let cv_semitones = (pitch_cv * 12.0).clamp(-12.0, 12.0);
        let semitones = self.base_semitones + cv_semitones;
        let ratio = (2.0f32).powf(semitones / 12.0);
        self.shared.shift_ratio.store(ratio);

        // mix CV: added to base mix, clamped to [0, 1].
        let mix = (self.base_mix + mix_cv).clamp(0.0, 1.0);
        self.shared.mix.store(mix);
    }
}
