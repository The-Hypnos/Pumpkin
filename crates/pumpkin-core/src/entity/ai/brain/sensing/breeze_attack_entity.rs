use std::sync::Arc;

use pumpkin_util::GameMode;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::{NearestLivingEntitySensor, Sensor, is_entity_attackable};

const REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_ATTACKABLE.id(),
];

/// Vanilla `BreezeAttackEntitySensor`: the living-entity scan, plus the first entity in it
/// the breeze may attack, skipping creative and spectator players.
pub struct BreezeAttackEntitySensor;

impl Sensor for BreezeAttackEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        NearestLivingEntitySensor.do_tick(tick);
        let attackable = {
            let ctx = tick.visibility();
            ctx.brain
                .get(types::NEAREST_LIVING_ENTITIES)
                .and_then(|entities| {
                    entities
                        .iter()
                        .find(|entity| {
                            let creative_or_spectator = entity.get_player().is_some_and(|player| {
                                matches!(
                                    player.gamemode.load(),
                                    GameMode::Creative | GameMode::Spectator
                                )
                            });
                            !creative_or_spectator && is_entity_attackable(&ctx, entity.as_ref())
                        })
                        .map(Arc::clone)
                })
        };
        tick.brain
            .set_optional(types::NEAREST_ATTACKABLE, attackable);
    }
}
