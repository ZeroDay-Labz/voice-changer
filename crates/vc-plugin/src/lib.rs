//! Voice Changer as a CLAP (and optionally VST3) plugin.

use nice_plug::context::gui::GuiContext;
use nice_plug::editor::dpi::NativeSize;
use nice_plug::prelude::*;
use nice_plug_iced::iced::{PollSubNotifier, Subscription, Task};
use nice_plug_iced::{IcedEditor, IcedEditorState, IcedNiceSettings, create_iced_editor};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use vc_core::{Meters, Pipeline, VcParams};
use vc_gui::{Capabilities, Host, HostSetting, Mode, Model};

const EDITOR_WIDTH: u32 = 1000;
const EDITOR_HEIGHT: u32 = 720;

pub struct VoiceChangerPlugin {
    params: Arc<VcParams>,
    pipeline: Pipeline,
    editor_state: Arc<IcedEditorState>,
    /// Audio thread → editor: "something changed, redraw".
    notifier: PollSubNotifier,
    /// Shared with the editor so it can display the current added latency.
    latency_ms: Arc<AtomicF32>,
    meters: Arc<Meters>,
}

impl Default for VoiceChangerPlugin {
    fn default() -> Self {
        let params = VcParams::new();
        Self {
            pipeline: Pipeline::new(48_000.0, 2048, params.clone()),
            params,
            editor_state: IcedEditorState::from_size(
                NativeSize::new(EDITOR_WIDTH, EDITOR_HEIGHT),
                1.0,
            ),
            notifier: PollSubNotifier::new(),
            latency_ms: Arc::new(AtomicF32::new(0.0)),
            meters: Arc::new(Meters::default()),
        }
    }
}

/// What the shared UI sees when it runs inside a DAW.
struct PluginHost {
    params: Arc<VcParams>,
    gui: GuiContext,
    meters: Arc<Meters>,
    latency_ms: Arc<AtomicF32>,
    #[cfg(feature = "ai")]
    ai_status: Arc<vc_core::ai::AiStatus>,
    theme: Cell<Mode>,
}

impl Host for PluginHost {
    fn params(&self) -> &VcParams {
        &self.params
    }
    fn gui(&self) -> &GuiContext {
        &self.gui
    }
    fn meters(&self) -> &Meters {
        &self.meters
    }
    fn theme_mode(&self) -> Mode {
        self.theme.get()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn latency_ms(&self) -> f32 {
        self.latency_ms.load(Ordering::Relaxed)
    }
    #[cfg(feature = "ai")]
    fn ai_status(&self) -> Option<Arc<vc_core::ai::AiStatus>> {
        Some(self.ai_status.clone())
    }
    fn apply_setting(&self, setting: HostSetting) {
        if let HostSetting::Theme(m) = setting {
            self.theme.set(m);
        }
    }
}

struct Editor {
    model: Model,
    host: PluginHost,
}

#[derive(Debug, Clone)]
enum EditorMessage {
    Ui(vc_gui::Message),
    Tick,
}

fn editor_update(editor: &mut Editor, message: EditorMessage) -> Task<EditorMessage> {
    let m = match message {
        EditorMessage::Ui(m) => m,
        EditorMessage::Tick => vc_gui::Message::Tick,
    };
    vc_gui::update(&mut editor.model, m, &editor.host).map(EditorMessage::Ui)
}

fn editor_view(editor: &Editor) -> nice_plug_iced::iced::Element<'_, EditorMessage> {
    vc_gui::view(&editor.model, &editor.host).map(EditorMessage::Ui)
}

impl Plugin for VoiceChangerPlugin {
    const NAME: &'static str = "Voice Changer";
    const VENDOR: &'static str = "ZeroDay-Labz";
    const URL: &'static str = "https://github.com/ZeroDay-Labz/voice-changer";
    const EMAIL: &'static str = "matnd3000@gmail.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: Some(new_nonzero_u32(1)),
            main_output_channels: Some(new_nonzero_u32(1)),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: Some(new_nonzero_u32(2)),
            main_output_channels: Some(new_nonzero_u32(2)),
            ..AudioIOLayout::const_default()
        },
    ];

    type Editor = IcedEditor;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        let params = self.params.clone();
        let meters = self.meters.clone();
        let latency_ms = self.latency_ms.clone();
        #[cfg(feature = "ai")]
        let ai_status = self.pipeline.ai_status();
        create_iced_editor(
            self.editor_state.clone(),
            (),
            self.notifier.clone(),
            IcedNiceSettings::new().with_tile(Self::NAME),
            move |pstate, ctx| {
                let params = params.clone();
                let meters = meters.clone();
                let latency_ms = latency_ms.clone();
                #[cfg(feature = "ai")]
                let ai_status = ai_status.clone();
                Ok(nice_plug_iced::application(
                    pstate,
                    ctx,
                    move |_pstate, ctx: nice_plug_iced::IcedNiceContext| {
                        let host = PluginHost {
                            params: params.clone(),
                            gui: ctx.nice_context.clone(),
                            meters: meters.clone(),
                            latency_ms: latency_ms.clone(),
                            #[cfg(feature = "ai")]
                            ai_status: ai_status.clone(),
                            theme: Cell::new(Mode::Dark),
                        };
                        (
                            Editor {
                                model: Model::new(),
                                host,
                            },
                            Task::none(),
                        )
                    },
                    editor_update,
                    editor_view,
                )
                .theme(|editor: &Editor| vc_gui::theme::theme(editor.host.theme_mode()))
                .subscription(|_editor: &Editor| -> Subscription<EditorMessage> {
                    nice_plug_iced::iced::poll_events().map(|_| EditorMessage::Tick)
                })
                .antialiasing(true)
                .run())
            },
        )
    }

    fn activate(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        // Keep the pipeline (and its loaded AI models) across re-activations;
        // only rebuild when the sample rate actually changes.
        if (self.pipeline.sample_rate() - buffer_config.sample_rate).abs() > 0.5 {
            self.pipeline = Pipeline::new(
                buffer_config.sample_rate,
                buffer_config.max_buffer_size as usize,
                self.params.clone(),
            );
        }
        self.pipeline.reset();
        self.pipeline.take_latency_changed();
        self.publish_latency(|n| context.set_latency_samples(n));
        true
    }

    fn reset(&mut self) {
        self.pipeline.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        if self.pipeline.take_latency_changed() {
            // CLAP only allows reporting latency from `activate`, so ask the
            // host to restart us; `activate` then reports the new value.
            self.publish_latency(|_| {});
            context.request_restart();
        }

        let channels = buffer.as_slice();
        match channels.len() {
            0 => {}
            1 => self.pipeline.process(channels[0]),
            n => {
                // Voice is mono: fold all inputs down, process once, fan out.
                let scale = 1.0 / n as f32;
                let (first, rest) = channels.split_at_mut(1);
                let mono = &mut *first[0];
                for other in rest.iter() {
                    for (m, &x) in mono.iter_mut().zip(other.iter()) {
                        *m += x;
                    }
                }
                for m in mono.iter_mut() {
                    *m *= scale;
                }
                self.pipeline.process(mono);
                for other in rest.iter_mut() {
                    other.copy_from_slice(mono);
                }
            }
        }
        let (i, o) = self.pipeline.peaks();
        self.meters.publish(i, o, true);
        self.meters.publish_auto_gain(self.pipeline.auto_gain_db());
        self.notifier.notify();
        ProcessStatus::Normal
    }
}

impl VoiceChangerPlugin {
    fn publish_latency(&mut self, report: impl FnOnce(u32)) {
        let samples = self.pipeline.latency_samples();
        self.latency_ms.store(
            samples as f32 / self.pipeline.sample_rate() * 1000.0,
            Ordering::Relaxed,
        );
        report(samples);
    }
}

impl ClapPlugin for VoiceChangerPlugin {
    const CLAP_ID: &'static str = "com.echo.voice-changer";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Realtime voice changer: pitch, formant, effects and local AI voices");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::PitchShifter,
        ClapFeature::Mono,
        ClapFeature::Stereo,
    ];
}

#[cfg(feature = "vst3")]
impl Vst3Plugin for VoiceChangerPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"echoVoiceChangr1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

nice_export_clap!(VoiceChangerPlugin);
#[cfg(feature = "vst3")]
nice_export_vst3!(VoiceChangerPlugin);
