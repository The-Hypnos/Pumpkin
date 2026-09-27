use pumpkin_data::entity::EntityType;

use super::super::memory::{MemoryModuleId, types};
use super::super::{Brain, BrainTick};
use super::Sensor;

pub const GOLEM_SCAN_RATE: i32 = 200;
const MEMORY_TIME_TO_LIVE: i64 = 599;

const REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_LIVING_ENTITIES.id(),
    types::GOLEM_DETECTED_RECENTLY.id(),
];

pub struct GolemSensor;

impl Sensor for GolemSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        check_for_nearby_golem(tick.brain);
    }
}

pub fn check_for_nearby_golem(brain: &mut Brain) {
    let golem_present = brain
        .get(types::NEAREST_LIVING_ENTITIES)
        .is_some_and(|entities| {
            entities
                .iter()
                .any(|entity| entity.get_entity().entity_type == &EntityType::IRON_GOLEM)
        });
    if golem_present {
        golem_detected(brain);
    }
}

pub fn golem_detected(brain: &mut Brain) {
    brain.set_with_expiry(types::GOLEM_DETECTED_RECENTLY, true, MEMORY_TIME_TO_LIVE);
}
