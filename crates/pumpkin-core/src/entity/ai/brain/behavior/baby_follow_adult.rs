use std::sync::Arc;

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;
use crate::entity::mob::Mob;

use super::super::Brain;
use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::one_shot::OneShot;

/// Vanilla `BabyFollowAdult.create(followRange, speedModifier)`, following `NEAREST_VISIBLE_ADULT`.
#[must_use]
pub fn baby_follow_adult(
    min_follow_range: i32,
    max_follow_range: i32,
    speed_modifier: impl Fn(&dyn Mob) -> f32 + Send + Sync + 'static,
) -> OneShot {
    baby_follow(
        types::NEAREST_VISIBLE_ADULT.id(),
        |brain| brain.get(types::NEAREST_VISIBLE_ADULT).map(Arc::clone),
        min_follow_range,
        max_follow_range,
        speed_modifier,
        false,
    )
}

/// Vanilla `BabyFollowAdult.create` in full: follows whatever `nearest` reads from `memory`.
#[must_use]
pub fn baby_follow(
    memory: MemoryModuleId,
    nearest: fn(&Brain) -> Option<Arc<dyn EntityBase>>,
    min_follow_range: i32,
    max_follow_range: i32,
    speed_modifier: impl Fn(&dyn Mob) -> f32 + Send + Sync + 'static,
    target_eye: bool,
) -> OneShot {
    let too_far = f64::from(max_follow_range + 1);
    let close_enough = f64::from(min_follow_range);
    OneShot::with_required(
        "BabyFollowAdult",
        vec![
            (memory, MemoryStatus::ValuePresent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        vec![types::LOOK_TARGET.id()],
        move |tick| {
            if !is_baby(tick.mob) {
                return false;
            }
            let Some(adult) = nearest(tick.brain) else {
                return false;
            };
            let distance_squared = tick
                .mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&adult.get_entity().pos.load());
            if distance_squared >= too_far * too_far
                || distance_squared < close_enough * close_enough
            {
                return false;
            }
            tick.brain.set(
                types::LOOK_TARGET,
                Arc::new(EntityTracker::with_target_eye_height(
                    Arc::clone(&adult),
                    true,
                    target_eye,
                )) as Arc<dyn PositionTracker>,
            );
            let walk_tracker = Arc::new(EntityTracker::with_target_eye_height(
                adult, target_eye, target_eye,
            ));
            tick.brain.set(
                types::WALK_TARGET,
                WalkTarget::new(walk_tracker, speed_modifier(tick.mob), min_follow_range - 1),
            );
            true
        },
    )
}
