use nih_plug::prelude::*;
use nih_plug_vizia::ViziaState;
use sofar::{
    reader::{Filter, OpenOptions, Sofar},
    render::Renderer,
};
use std::{io::Cursor, sync::Arc};
mod editor;

const PARTITION_LEN: usize = 32;

static SOFA_DATA: &[u8] = include_bytes!("assets/mit_kemar_normal_pinna.sofa");

// parameters and gui state
#[derive(Params)]
struct HrtfConvParams {
    #[persist = "editor-state"]
    editor_state: Arc<ViziaState>,
    #[id = "azimuth"]
    pub azimuth: FloatParam,
    #[id = "elevation"]
    pub elevation: FloatParam,
    #[id = "distance"]
    pub distance: FloatParam,
}

impl Default for HrtfConvParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_state(),

            azimuth: FloatParam::new(
                "Azimuth",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 359.0,
                },
            )
            .with_unit("°")
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_step_size(0.01),
            elevation: FloatParam::new(
                "Elevation",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 180.0,
                },
            )
            .with_unit("°")
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_step_size(0.01),
            distance: FloatParam::new("Distance", 1.0, FloatRange::Linear { min: 0.1, max: 1.0 })
                .with_unit("m")
                .with_smoother(SmoothingStyle::Logarithmic(50.0))
                .with_step_size(0.05),
        }
    }
}

// plugin struct
struct HrtfConv {
    params: Arc<HrtfConvParams>,
    sofa: Option<Sofar>,
    filter: Option<Filter>,
    renderer: Option<Renderer>,
    scratch_buffer: Vec<f32>,
    last_direction: (f32, f32, f32),
}

impl Default for HrtfConv {
    // constructor
    fn default() -> Self {
        Self {
            params: Arc::new(HrtfConvParams::default()),
            sofa: None,
            filter: None,
            renderer: None,
            scratch_buffer: vec![],
            last_direction: (0.0, 0.0, 0.0),
        }
    }
}

impl Plugin for HrtfConv {
    const NAME: &'static str = "Hrtf Conv";
    const VENDOR: &'static str = "ph3nac";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "ph3nac@gmail.com";

    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // The first audio IO layout is used as the default. The other layouts may be selected either
    // explicitly or automatically by the host or the user depending on the plugin API/backend.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(1),
        main_output_channels: NonZeroU32::new(2),

        aux_input_ports: &[],
        aux_output_ports: &[],

        // Individual ports and the layout as a whole can be named here. By default these names
        // are generated as needed. This layout will be called 'Stereo', while a layout with
        // only one input and output channel would be called 'Mono'.
        names: PortNames::const_default(),
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    // If the plugin can send or receive SysEx messages, it can define a type to wrap around those
    // messages here. The type implements the `SysExMessage` trait, which allows conversion to and
    // from plain byte buffers.
    type SysExMessage = ();
    // More advanced plugins can use this to run expensive background tasks. See the field's
    // documentation for more information. `()` means that the plugin does not have any background
    // tasks.
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.params.editor_state.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let cursor = Cursor::new(SOFA_DATA);
        let sofa = OpenOptions::new()
            .sample_rate(buffer_config.sample_rate)
            .open_data(cursor.get_ref());
        if sofa.is_err() {
            nih_error!("Failed to open HRTF data");
            return false;
        }
        let sofa = sofa.unwrap();

        let filter_len = sofa.filter_len();

        let az_deg = self.params.azimuth.value();
        let el_deg = self.params.elevation.value();
        let dist = self.params.distance.value();
        let az = az_deg.to_radians();
        let el = el_deg.to_radians();
        let x = dist * (el.cos() * az.cos());
        let y = dist * (el.cos() * az.sin());
        let z = dist * el.sin();
        let current_direction = (x, y, z);

        let mut filter = Filter::new(filter_len);
        sofa.filter(x, y, z, &mut filter);

        let render = Renderer::builder(filter_len)
            .with_sample_rate(buffer_config.sample_rate)
            .with_partition_len(PARTITION_LEN)
            .build();
        if render.is_err() {
            nih_error!("Failed to create HRTF renderer");
            return false;
        }
        let mut render = render.unwrap();

        render.set_filter(&filter).expect("Failed to set filter");

        self.sofa = Some(sofa);
        self.filter = Some(filter);
        self.renderer = Some(render);
        self.last_direction = current_direction;

        self.scratch_buffer.clear();
        self.scratch_buffer
            .resize(buffer_config.max_buffer_size as usize, 0.0);

        // Resize buffers and perform other potentially expensive initialization operations here.
        // The `reset()` function is always called right after this function. You can remove this
        // function if you do not need it.
        true
    }

    fn reset(&mut self) {
        // Reset buffers and envelopes here. This can be called from the audio thread and may not
        // allocate. You can remove this function if you do not need it.
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let render = match &mut self.renderer {
            Some(r) => r,
            None => return ProcessStatus::Normal,
        };

        let az_deg = self.params.azimuth.value();
        let el_deg = self.params.elevation.value();
        let dist = self.params.distance.value();
        let az = az_deg.to_radians();
        let el = el_deg.to_radians();
        let x = dist * (el.cos() * az.cos());
        let y = dist * (el.cos() * az.sin());
        let z = dist * el.sin();
        let current_direction = (x, y, z);

        if current_direction != self.last_direction {
            if let Some(sofa) = self.sofa.as_mut() {
                if let Some(filter) = self.filter.as_mut() {
                    sofa.filter(x, y, z, filter);
                    if let Err(e) = render.set_filter(filter) {
                        nih_error!("Failed to set filter:{}", e);
                        return ProcessStatus::Error("HRTF processing failed");
                    }

                    self.last_direction = current_direction;
                }
            }
        }

        let num_samples = buffer.samples();
        let channels = buffer.as_slice();
        let num_channels = channels.len();

        if num_channels < 2 || num_samples == 0 {
            return ProcessStatus::Normal;
        }

        // no allocation here
        self.scratch_buffer.clear();
        self.scratch_buffer.extend_from_slice(channels[0]);

        let (left_chan, right_chan) = channels.split_at_mut(1);
        let left_out = &mut left_chan[0][..num_samples];
        let right_out = &mut right_chan[0][..num_samples];

        if let Err(e) = render.process_block(&self.scratch_buffer, left_out, right_out) {
            nih_error!("HRTF render error:{}", e);
            return ProcessStatus::Error("HRTF processing failed");
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for HrtfConv {
    const CLAP_ID: &'static str = "com.ph3nac.hrtf-conv";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A short description of your plugin");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;

    // Don't forget to change these features
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo];
}

impl Vst3Plugin for HrtfConv {
    const VST3_CLASS_ID: [u8; 16] = *b"Exactly16Chars!!";

    // And also don't forget to change these categories
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(HrtfConv);
nih_export_vst3!(HrtfConv);
