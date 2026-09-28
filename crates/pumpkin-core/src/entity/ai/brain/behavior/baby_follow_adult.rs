use std::sync::Arc;

use crate::entity::ageable::is_baby;
use crate::entity::mob::Mob;

use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryStatus, types};
use super::one_shot::OneShot;

/// Vanilla `BabyFollowAdult.create`, following `NEAREST_VISIBLE_ADULT`.
#[must_use]
pub fn baby_follow_adult(
    min_follow_range: i32,
    max_follow_range: i32,
    speed_modifier: impl Fn(&dyn Mob) -> f32 + Send + Sync + 'static,
) -> OneShot {
    let too_far = f64::from(max_follow_range + 1);
    let close_enough = f64::from(min_follow_range);
    OneShot::with_required(
        "BabyFollowAdult",
        vec![
            (
                types::NEAREST_VISIBLE_ADULT.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
        vec![types::LOOK_TARGET.id()],
        move |tick| {
            if !is_baby(tick.mob) {
                return false;
            }
            let Some(adult) = tick.brain.get(types::NEAREST_VISIBLE_ADULT).map(Arc::clone) else {
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
                Arc::new(EntityTracker::new(Arc::clone(&adult), true)) as Arc<dyn PositionTracker>,
            );
            tick.brain.set(
                types::WALK_TARGET,
                WalkTarget::from_entity(adult, speed_modifier(tick.mob), min_follow_range - 1),
            );
            true
        },
    )
}
