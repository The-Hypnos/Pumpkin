use std::sync::atomic::Ordering;

use rand::RngExt;

use crate::entity::ai::goal::swim::SwimGoal;
use crate::entity::mob::Mob;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryStatus};
use super::timed::Behavior;

/// Vanilla `Swim`: keeps jumping while deep in a floatable fluid or in lava.
pub struct Swim {
    chance: f32,
    can_start: fn(&dyn Mob) -> bool,
}

impl Swim {
    #[must_use]
    pub const fn new(chance: f32) -> Self {
        Self::with_condition(chance, |_| true)
    }

    /// A `Swim` with vanilla's anonymous-subclass extra start condition.
    #[must_use]
    pub const fn with_condition(chance: f32, can_start: fn(&dyn Mob) -> bool) -> Self {
        Self { chance, can_start }
    }

    fn should_swim(&self, mob: &dyn Mob) -> bool {
        (self.can_start)(mob) && SwimGoal::is_in_fluid(mob)
    }
}

impl Behavior for Swim {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.should_swim(tick.mob)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        self.should_swim(tick.mob)
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if tick.mob.get_random().random::<f32>() < self.chance {
            tick.mob
                .get_mob_entity()
                .living_entity
                .jumping
                .store(true, Ordering::SeqCst);
        }
    }

    fn debug_name(&self) -> &'static str {
        "Swim"
    }
}
