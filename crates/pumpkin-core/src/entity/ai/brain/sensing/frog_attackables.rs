use crate::entity::passive::frog_ai::can_eat;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::{Sensor, is_entity_attackable, set_nearest_visible_matching};

pub const TARGET_DETECTION_DISTANCE: f64 = 10.0;

const REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_ATTACKABLE.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::UNREACHABLE_TONGUE_TARGETS.id(),
];

/// Vanilla `FrogAttackablesSensor`: the nearest edible, reachable mob within ten blocks.
pub struct FrogAttackablesSensor;

impl Sensor for FrogAttackablesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let body_pos = tick.mob.get_entity().pos.load();
        set_nearest_visible_matching(tick, types::NEAREST_ATTACKABLE, |ctx, entity| {
            let unreachable = ctx
                .brain
                .get(types::UNREACHABLE_TONGUE_TARGETS)
                .is_some_and(|targets| targets.contains(&entity.get_entity().entity_uuid));
            is_entity_attackable(ctx, entity.as_ref())
                && can_eat(entity.as_ref())
                && !unreachable
                && entity
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&body_pos)
                    < TARGET_DETECTION_DISTANCE * TARGET_DETECTION_DISTANCE
        });
    }
}
