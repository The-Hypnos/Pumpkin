use std::sync::Arc;

use pumpkin_data::tag::{self, Taggable};

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;

const REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_VISIBLE_ADULT.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
];

/// Vanilla `AdultSensor`: the nearest visible adult of the body's own type.
pub struct AdultSensor;

/// Vanilla `AdultSensorAnyType`: the nearest visible adult of any followable friendly type.
pub struct AdultSensorAnyType;

impl Sensor for AdultSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let body_type = tick.mob.get_entity().entity_type;
        set_nearest_visible_adult(tick, |entity| {
            entity.get_entity().entity_type == body_type && !is_baby(entity.as_ref())
        });
    }
}

impl Sensor for AdultSensorAnyType {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        set_nearest_visible_adult(tick, |entity| {
            entity
                .get_entity()
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_FOLLOWABLE_FRIENDLY_MOBS)
                && !is_baby(entity.as_ref())
        });
    }
}

// Vanilla leaves the memory untouched when NEAREST_VISIBLE_LIVING_ENTITIES is absent.
fn set_nearest_visible_adult(
    tick: &mut BrainTick<'_>,
    filter: impl Fn(&Arc<dyn EntityBase>) -> bool,
) {
    let adult = {
        let ctx = tick.visibility();
        let Some(visible) = ctx.brain.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            return;
        };
        visible.find_closest(&ctx, filter)
    };
    tick.brain.set_optional(types::NEAREST_VISIBLE_ADULT, adult);
}
