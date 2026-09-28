use std::sync::Arc;

use crate::entity::EntityBase;
use crate::entity::mob::Mob;

use super::super::BrainTick;
use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::{Behavior, NO_TIMEOUT};

pub const TEMPTATION_COOLDOWN: i32 = 100;
pub const DEFAULT_CLOSE_ENOUGH_DIST: f64 = 2.5;
pub const BACKED_UP_CLOSE_ENOUGH_DIST: f64 = 3.5;

type MobFn<T> = Box<dyn Fn(&dyn Mob) -> T + Send + Sync>;

/// Vanilla `FollowTemptation`: walks to the `TEMPTING_PLAYER` until they stop tempting.
pub struct FollowTemptation {
    conditions: [(MemoryModuleId, MemoryStatus); 6],
    speed_modifier: MobFn<f32>,
    close_enough_distance: MobFn<f64>,
    look_in_the_eyes: bool,
}

impl FollowTemptation {
    #[must_use]
    pub fn new(speed_modifier: impl Fn(&dyn Mob) -> f32 + Send + Sync + 'static) -> Self {
        Self::with_distance(speed_modifier, |_| DEFAULT_CLOSE_ENOUGH_DIST, false)
    }

    #[must_use]
    pub fn with_distance(
        speed_modifier: impl Fn(&dyn Mob) -> f32 + Send + Sync + 'static,
        close_enough_distance: impl Fn(&dyn Mob) -> f64 + Send + Sync + 'static,
        look_in_the_eyes: bool,
    ) -> Self {
        Self {
            conditions: [
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (types::WALK_TARGET.id(), MemoryStatus::Registered),
                (
                    types::TEMPTATION_COOLDOWN_TICKS.id(),
                    MemoryStatus::ValueAbsent,
                ),
                (types::TEMPTING_PLAYER.id(), MemoryStatus::ValuePresent),
                (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            ],
            speed_modifier: Box::new(speed_modifier),
            close_enough_distance: Box::new(close_enough_distance),
            look_in_the_eyes,
        }
    }
}

impl Behavior for FollowTemptation {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    // Vanilla overrides `timedOut` to never time out.
    fn min_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn max_duration(&self) -> i32 {
        NO_TIMEOUT
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::TEMPTING_PLAYER.id())
            && !tick.brain.has_memory_value(types::BREED_TARGET.id())
            && !tick.brain.has_memory_value(types::IS_PANICKING.id())
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(player) = tick.brain.get(types::TEMPTING_PLAYER).map(Arc::clone) else {
            return;
        };
        let player = player as Arc<dyn EntityBase>;
        tick.brain.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(Arc::clone(&player), true)) as Arc<dyn PositionTracker>,
        );
        let close_enough = (self.close_enough_distance)(tick.mob);
        let distance_squared = tick
            .mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&player.get_entity().pos.load());
        if distance_squared < close_enough * close_enough {
            tick.brain.erase(types::WALK_TARGET.id());
        } else {
            let tracker = Arc::new(EntityTracker::with_target_eye_height(
                player,
                self.look_in_the_eyes,
                self.look_in_the_eyes,
            ));
            tick.brain.set(
                types::WALK_TARGET,
                WalkTarget::new(tracker, (self.speed_modifier)(tick.mob), 2),
            );
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain
            .set(types::TEMPTATION_COOLDOWN_TICKS, TEMPTATION_COOLDOWN);
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::LOOK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "FollowTemptation"
    }
}
