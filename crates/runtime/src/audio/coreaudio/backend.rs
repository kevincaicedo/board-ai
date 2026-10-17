//! The device supervisor's CoreAudio side: snapshots from the HAL, units from VoiceProcessingIO.

use std::sync::Arc;

use super::device::snapshot;
use super::unit::VoiceProcessingUnit;
use crate::audio::AudioCounters;
use crate::audio::AudioError;
use crate::audio::devices::{DeviceSnapshot, Route};
use crate::audio::supervisor::{AudioBackend, UnitFacts, UnitParts};

pub(crate) struct CoreAudioBackend {
    counters: Arc<AudioCounters>,
}

impl CoreAudioBackend {
    pub(crate) const fn new(counters: Arc<AudioCounters>) -> Self {
        Self { counters }
    }
}

impl AudioBackend for CoreAudioBackend {
    type Unit = VoiceProcessingUnit;

    fn snapshot(&mut self) -> DeviceSnapshot {
        snapshot()
    }

    fn start(
        &mut self,
        route: &Route,
        parts: UnitParts,
    ) -> Result<(Self::Unit, UnitFacts), (AudioError, UnitParts)> {
        let unit = VoiceProcessingUnit::start(route, parts, Arc::clone(&self.counters))?;
        let facts = unit.facts();
        Ok((unit, facts))
    }

    fn stop(&mut self, unit: Self::Unit) -> UnitParts {
        unit.stop()
    }
}
