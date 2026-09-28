use pumpkin_data::entity::EntityType;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, NearestVisibleLivingEntities, types};
use super::Sensor;

const REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_HOSTILE.id(),
];

/// Vanilla `VillagerHostilesSensor.ACCEPTABLE_DISTANCE_FROM_HOSTILES`: how close each hostile
/// has to be before a villager counts it.
#[must_use]
pub fn acceptable_distance_from_hostile(entity_type: &EntityType) -> Option<f32> {
    Some(match entity_type {
        t if t == &EntityType::DROWNED => 8.0,
        t if t == &EntityType::EVOKER => 12.0,
        t if t == &EntityType::HUSK => 8.0,
        t if t == &EntityType::ILLUSIONER => 12.0,
        t if t == &EntityType::PILLAGER => 15.0,
        t if t == &EntityType::RAVAGER => 12.0,
        t if t == &EntityType::VEX => 8.0,
        t if t == &EntityType::VINDICATOR => 10.0,
        t if t == &EntityType::ZOGLIN => 10.0,
        t if t == &EntityType::ZOMBIE => 8.0,
        t if t == &EntityType::ZOMBIE_VILLAGER => 8.0,
        _ => return None,
    })
}

/// Vanilla `VillagerHostilesSensor`: the nearest visible hostile close enough to flee from.
pub struct VillagerHostilesSensor;

impl Sensor for VillagerHostilesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let body_pos = tick.mob.get_entity().pos.load();
        let hostile = {
            let ctx = tick.visibility();
            let empty = NearestVisibleLivingEntities::empty();
            ctx.brain
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .unwrap_or(&empty)
                .find_closest(&ctx, |mob| {
                    let entity = mob.get_entity();
                    acceptable_distance_from_hostile(entity.entity_type).is_some_and(|max| {
                        entity.pos.load().squared_distance_to_vec(&body_pos) <= f64::from(max * max)
                    })
                })
        };
        tick.brain.set_optional(types::NEAREST_HOSTILE, hostile);
    }
}
