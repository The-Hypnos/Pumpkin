use pumpkin_data::entity::EntityType;

use crate::entity::ageable::is_baby;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;

const REQUIRES: &[MemoryModuleId] = &[types::VISIBLE_VILLAGER_BABIES.id()];

/// Vanilla `VillagerBabiesSensor`: every visible baby villager.
pub struct VillagerBabiesSensor;

impl Sensor for VillagerBabiesSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let babies = {
            let ctx = tick.visibility();
            ctx.brain
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .map(|visible| {
                    visible
                        .find_all(&ctx, |entity| {
                            entity.get_entity().entity_type == &EntityType::VILLAGER
                                && is_baby(entity.as_ref())
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        tick.brain.set(types::VISIBLE_VILLAGER_BABIES, babies);
    }
}
