use std::sync::Arc;

use super::super::BrainTick;
use super::super::memory::position_tracker::PositionTracker;
use super::super::memory::types;
use super::super::memory::walk_target::WalkTarget;
use super::one_shot::OneShot;

/// Vanilla `StayCloseToTarget`: walks back toward the target once it is `too_far` away.
#[must_use]
pub fn stay_close_to_target(
    target_position: impl Fn(&BrainTick<'_>) -> Option<Arc<dyn PositionTracker>> + Send + Sync + 'static,
    should_run: impl Fn(&BrainTick<'_>) -> bool + Send + Sync + 'static,
    close_enough: i32,
    too_far: i32,
    speed_modifier: f32,
) -> OneShot {
    let too_far_squared = f64::from(too_far * too_far);
    OneShot::with_required(
        "StayCloseToTarget",
        Vec::new(),
        vec![types::LOOK_TARGET.id(), types::WALK_TARGET.id()],
        move |tick| {
            let Some(target) = target_position(tick) else {
                return false;
            };
            if !should_run(tick) {
                return false;
            }
            let body_pos = tick.mob.get_entity().pos.load();
            if body_pos.squared_distance_to_vec(&target.current_position()) < too_far_squared {
                return false;
            }
            tick.brain.set(types::LOOK_TARGET, Arc::clone(&target));
            tick.brain.set(
                types::WALK_TARGET,
                WalkTarget::new(target, speed_modifier, close_enough),
            );
            true
        },
    )
}
