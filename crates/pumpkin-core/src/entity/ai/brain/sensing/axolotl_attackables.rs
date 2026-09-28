use pumpkin_data::tag::{self, Taggable};

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::{Sensor, is_entity_attackable, set_nearest_visible_matching};

pub const TARGET_DETECTION_DISTANCE: f64 = 8.0;

const REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_ATTACKABLE.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::HAS_HUNTING_COOLDOWN.id(),
];

/// Vanilla `AxolotlAttackablesSensor`: the nearest hostile, or prey off hunting cooldown,
/// swimming within eight blocks.
pub struct AxolotlAttackablesSensor;

impl Sensor for AxolotlAttackablesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let body_pos = tick.mob.get_entity().pos.load();
        set_nearest_visible_matching(tick, types::NEAREST_ATTACKABLE, |ctx, entity| {
            let target = entity.get_entity();
            let is_close = target.pos.load().squared_distance_to_vec(&body_pos)
                <= TARGET_DETECTION_DISTANCE * TARGET_DETECTION_DISTANCE;
            let is_hostile = target
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_ALWAYS_HOSTILES);
            let is_hunt_target = !ctx.brain.has_memory_value(types::HAS_HUNTING_COOLDOWN.id())
                && target
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_HUNT_TARGETS);
            is_close
                && target.is_in_water()
                && (is_hostile || is_hunt_target)
                && is_entity_attackable(ctx, entity.as_ref())
        });
    }
}
