use crate::entity::EntityBase;
use crate::entity::mob::Mob;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryModuleType, types};
use super::Sensor;

/// Vanilla `MobSensor`: while `ready_test` holds, remembers `to_set` for `memory_time_to_live`
/// ticks whenever a nearby entity passes `mob_test`; otherwise forgets it.
pub struct MobSensor {
    requires: &'static [MemoryModuleId],
    mob_test: fn(&dyn Mob, &dyn EntityBase) -> bool,
    ready_test: fn(&dyn Mob) -> bool,
    to_set: MemoryModuleType<bool>,
    memory_time_to_live: i64,
}

impl MobSensor {
    #[must_use]
    pub const fn new(
        requires: &'static [MemoryModuleId],
        mob_test: fn(&dyn Mob, &dyn EntityBase) -> bool,
        ready_test: fn(&dyn Mob) -> bool,
        to_set: MemoryModuleType<bool>,
        memory_time_to_live: i64,
    ) -> Self {
        Self {
            requires,
            mob_test,
            ready_test,
            to_set,
            memory_time_to_live,
        }
    }
}

impl Sensor for MobSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        self.requires
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        if !(self.ready_test)(tick.mob) {
            tick.brain.erase(self.to_set.id());
            return;
        }
        let mob_present = tick
            .brain
            .get(types::NEAREST_LIVING_ENTITIES)
            .is_some_and(|entities| {
                entities
                    .iter()
                    .any(|entity| (self.mob_test)(tick.mob, entity.as_ref()))
            });
        if mob_present {
            tick.brain
                .set_with_expiry(self.to_set, true, self.memory_time_to_live);
        }
    }
}
