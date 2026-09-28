use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryModuleType, MemoryStatus};
use super::timed::{Behavior, NO_TIMEOUT};

/// Vanilla `CountDownCooldownTicks`: decrements an integer memory each tick, erasing it at zero.
pub struct CountDownCooldownTicks {
    conditions: [(MemoryModuleId, MemoryStatus); 1],
    cooldown_ticks: MemoryModuleType<i32>,
}

impl CountDownCooldownTicks {
    #[must_use]
    pub const fn new(cooldown_ticks: MemoryModuleType<i32>) -> Self {
        Self {
            conditions: [(cooldown_ticks.id(), MemoryStatus::ValuePresent)],
            cooldown_ticks,
        }
    }
}

impl Behavior for CountDownCooldownTicks {
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
        tick.brain
            .get(self.cooldown_ticks)
            .is_some_and(|ticks| *ticks > 0)
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(ticks) = tick.brain.get(self.cooldown_ticks).copied() {
            tick.brain.set(self.cooldown_ticks, ticks - 1);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(self.cooldown_ticks.id());
    }

    fn debug_name(&self) -> &'static str {
        "CountDownCooldownTicks"
    }
}
