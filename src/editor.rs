use nih_plug::editor::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::{ParamSlider, ParamSliderExt};
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::HrtfConvParams;

#[derive(Lens)]
struct Data {
    params: Arc<HrtfConvParams>,
}

impl Model for Data {}

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (400, 300))
}

pub(crate) fn create(
    params: Arc<HrtfConvParams>,
    editor_state: Arc<ViziaState>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::default(), move |cx, _| {
        Data {
            params: params.clone(),
        }
        .build(cx);

        VStack::new(cx, |cx| {
            ParamSlider::new(cx, Data::params, |params| &params.azimuth)
                .with_label("az")
                .border_radius("10")
                .size(Stretch(10.0));
            ParamSlider::new(cx, Data::params, |params| &params.elevation)
                .with_label("el")
                .border_radius("10")
                .size(Stretch(10.0));
            ParamSlider::new(cx, Data::params, |params| &params.distance)
                .with_label("distance")
                .border_radius("10")
                .size(Stretch(10.0));
        });
        // ResizeHandle::new(cx);
    })
}
