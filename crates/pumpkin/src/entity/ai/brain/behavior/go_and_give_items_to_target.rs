use std::sync::Arc;

use pumpkin_util::math::vector3::Vector3;

use super::super::BrainTick;
use super::super::memory::position_tracker::PositionTracker;
use super::super::memory::{MemoryModuleId, MemoryModuleType, MemoryStatus, types};
use super::timed::Behavior;
use super::utils::set_walk_and_look_target_memories;

const CLOSE_ENOUGH_DISTANCE_TO_TARGET: f64 = 3.0;

pub type TargetPositionGetter = fn(&BrainTick<'_>) -> Option<Arc<dyn PositionTracker>>;
pub type ItemThrower = fn(&mut BrainTick<'_>, Vector3<f64>);

/// Vanilla `GoAndGiveItemsToTarget`: carries held items to a target and throws them once close.
pub struct GoAndGiveItemsToTarget {
    conditions: [(MemoryModuleId, MemoryStatus); 3],
    target_position: TargetPositionGetter,
    speed_modifier: f32,
    timeout: i32,
    item_thrower: ItemThrower,
    cooldown_memory: MemoryModuleType<i32>,
    cooldown_duration: i32,
    has_item: fn(&BrainTick<'_>) -> bool,
}

impl GoAndGiveItemsToTarget {
    #[must_use]
    pub const fn new(
        target_position: TargetPositionGetter,
        speed_modifier: f32,
        timeout: i32,
        item_thrower: ItemThrower,
        cooldown_memory: MemoryModuleType<i32>,
        cooldown_duration: i32,
        has_item: fn(&BrainTick<'_>) -> bool,
    ) -> Self {
        Self {
            conditions: [
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (types::WALK_TARGET.id(), MemoryStatus::Registered),
                (cooldown_memory.id(), MemoryStatus::Registered),
            ],
            target_position,
            speed_modifier,
            timeout,
            item_thrower,
            cooldown_memory,
            cooldown_duration,
            has_item,
        }
    }

    fn can_throw_item_to_target(&self, tick: &BrainTick<'_>) -> bool {
        (self.has_item)(tick) && (self.target_position)(tick).is_some()
    }
}

impl Behavior for GoAndGiveItemsToTarget {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn min_duration(&self) -> i32 {
        self.timeout
    }

    fn max_duration(&self) -> i32 {
        self.timeout
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.can_throw_item_to_target(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        self.can_throw_item_to_target(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(target) = (self.target_position)(tick) {
            set_walk_and_look_target_memories(
                tick.brain,
                target,
                self.speed_modifier,
                CLOSE_ENOUGH_DISTANCE_TO_TARGET as i32,
            );
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = (self.target_position)(tick) else {
            return;
        };
        let deposit_position = target.current_position();
        let eye_pos = tick.mob.get_entity().get_eye_pos();
        if (deposit_position - eye_pos).length() < CLOSE_ENOUGH_DISTANCE_TO_TARGET {
            (self.item_thrower)(tick, target.current_position());
            tick.brain.set(self.cooldown_memory, self.cooldown_duration);
        }
    }

    fn debug_name(&self) -> &'static str {
        "GoAndGiveItemsToTarget"
    }
}
