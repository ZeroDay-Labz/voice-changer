//! A `GuiContextInner` for running the shared UI without a plugin host.

use nice_plug::plugin::ParamValue;
use nice_plug::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use vc_core::Preset;

use crate::control::Shared;

pub struct StandaloneGuiContext {
    pub shared: Arc<Shared>,
}

impl GuiContextInner for StandaloneGuiContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Standalone
    }

    unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}

    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        self.shared.set_param_normalized(param, normalized);
    }

    unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}

    fn get_state(&self) -> PluginState {
        let snapshot = Preset::capture(&self.shared.params, "state");
        PluginState {
            version: env!("CARGO_PKG_VERSION").to_string(),
            params: snapshot
                .values
                .into_iter()
                .map(|(id, v)| (id, ParamValue::F32(v)))
                .collect(),
            fields: BTreeMap::new(),
        }
    }

    fn set_state(&self, state: PluginState) {
        let values = state
            .params
            .into_iter()
            .filter_map(|(id, v)| match v {
                ParamValue::F32(f) => Some((id, f)),
                ParamValue::I32(i) => Some((id, i as f32)),
                ParamValue::Bool(b) => Some((id, b as u8 as f32)),
                ParamValue::String(_) => None,
            })
            .collect();
        let preset = Preset {
            name: "state".into(),
            description: String::new(),
            values,
            ai_voice: None,
        };
        self.shared.apply_preset(&preset);
    }

    fn request_restart(&self) {}
}
