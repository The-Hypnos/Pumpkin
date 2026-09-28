use std::sync::Arc;

use crate::entity::mob::Mob;

use super::super::BrainTick;
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryStatus, types};
use super::one_shot::OneShot;

#[must_use]
pub fn set_walk_target_from_look_target(
    can_set_walk_target: impl Fn(&BrainTick<'_>) -> bool + Send + Sync + 'static,
    speed_modifier: f32,
    close_enough_dist: i32,
) -> OneShot {
    set_walk_target_from_look_target_with_speed(
        can_set_walk_target,
        move |_| speed_modifier,
        close_enough_dist,
    )
}

/// As above, with the speed chosen per mob each time, like vanilla's `Function<LivingEntity, Float>`.
#[must_use]
pub fn set_walk_target_from_look_target_with_speed(
    can_set_walk_target: impl Fn(&BrainTick<'_>) -> bool + Send + Sync + 'static,
    speed_modifier: impl Fn(&dyn Mob) -> f32 + Send + Sync + 'static,
    close_enough_dist: i32,
) -> OneShot {
    OneShot::new(
        "SetWalkTargetFromLookTarget",
        vec![
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::LOOK_TARGET.id(), MemoryStatus::ValuePresent),
        ],
        move |tick| {
            if !can_set_walk_target(tick) {
                return false;
            }
            let Some(look_target) = tick.brain.get(types::LOOK_TARGET).map(Arc::clone) else {
                return false;
            };
            let speed = speed_modifier(tick.mob);
            tick.brain.set(
                types::WALK_TARGET,
                WalkTarget::new(look_target, speed, close_enough_dist),
            );
            true
        },
    )
}
