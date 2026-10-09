//! The complete per-host processing chain: optional AI conversion followed
//! by the DSP engine. Hosts (plugin, app) own one of these per stream.

use crate::VcParams;
use std::sync::Arc;
use vc_dsp::Engine;

pub struct Pipeline {
    pub engine: Engine,
    params: Arc<VcParams>,
    #[cfg(feature = "ai")]
    ai: crate::ai::AiStage,
    #[cfg(feature = "ai")]
    _ai_handle: crate::ai::AiHandle,
}

impl Pipeline {
    pub fn new(sample_rate: f32, max_block: usize, params: Arc<VcParams>) -> Self {
        let mut engine = Engine::new(sample_rate, max_block);
        engine.set_params(params.snapshot());
        engine.reset();
        #[cfg(feature = "ai")]
        let (ai, handle) = crate::ai::AiWorker::spawn(
            params.clone(),
            crate::ai::StreamConfig {
                sample_rate: sample_rate.round() as usize,
                ..Default::default()
            },
        );
        Self {
            engine,
            params,
            #[cfg(feature = "ai")]
            ai,
            #[cfg(feature = "ai")]
            _ai_handle: handle,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.engine.sample_rate()
    }

    pub fn reset(&mut self) {
        self.engine.reset();
    }

    /// Total added latency in samples.
    pub fn latency_samples(&self) -> u32 {
        #[allow(unused_mut)]
        let mut total = self.engine.latency_samples();
        #[cfg(feature = "ai")]
        if self.ai.is_active() {
            total += self.ai.latency_samples() as u32;
        }
        total
    }

    /// True once after anything changed the latency.
    pub fn take_latency_changed(&mut self) -> bool {
        #[allow(unused_mut)]
        let mut changed = self.engine.take_latency_changed();
        #[cfg(feature = "ai")]
        {
            changed |= self.ai.take_active_changed();
        }
        changed
    }

    #[cfg(feature = "ai")]
    pub fn ai_status(&self) -> Arc<crate::ai::AiStatus> {
        self.ai.status()
    }

    /// Process one mono block in place. Realtime-safe.
    pub fn process(&mut self, buf: &mut [f32]) {
        #[allow(unused_mut)]
        let mut snapshot = self.params.snapshot();
        #[cfg(feature = "ai")]
        {
            snapshot.skip_voice_fx = self.ai.is_active() && self.params.ai_solo.value();
        }
        self.engine.set_params(snapshot);
        #[cfg(feature = "ai")]
        {
            let ai = &mut self.ai;
            self.engine.process_with(buf, |b| {
                ai.process(b);
            });
        }
        #[cfg(not(feature = "ai"))]
        self.engine.process(buf);
    }

    pub fn peaks(&self) -> (f32, f32) {
        self.engine.peaks()
    }

    pub fn auto_gain_db(&self) -> f32 {
        self.engine.auto_gain_db()
    }
}
