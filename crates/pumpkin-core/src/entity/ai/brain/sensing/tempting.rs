use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::GameMode;

use crate::entity::EntityBase;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;
use crate::entity::player::Player;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;

const REQUIRES: &[MemoryModuleId] = &[types::TEMPTING_PLAYER.id()];

pub type Temptations = fn(&dyn Mob, &ItemStack) -> bool;

pub struct TemptingSensor {
    temptations: Temptations,
}

impl TemptingSensor {
    #[must_use]
    pub const fn new(temptations: Temptations) -> Self {
        Self { temptations }
    }

    #[must_use]
    pub fn for_animal() -> Self {
        Self::new(|mob, stack| mob.as_animal().is_some_and(|animal| animal.is_food(stack)))
    }

    fn player_holding_temptation(&self, mob: &dyn Mob, player: &Player) -> bool {
        let inventory = player.inventory();
        (self.temptations)(mob, &inventory.held_item())
            || (self.temptations)(mob, &inventory.off_hand_item())
    }
}

impl Sensor for TemptingSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let body = tick.mob;
        let body_entity = body.get_entity();
        // Vanilla narrows the attribute to a float before ranging the conditions.
        let range = body
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::TEMPT_RANGE) as f32;
        let targeting = TargetPredicate::create_non_attackable()
            .ignore_visibility()
            .set_base_max_distance(f64::from(range));
        let body_pos = body_entity.pos.load();

        let player = tick
            .world
            .players
            .load()
            .iter()
            .filter(|player| player.gamemode.load() != GameMode::Spectator)
            .filter(|player| targeting.test(tick.world, Some(body), player.as_ref()))
            .filter(|player| self.player_holding_temptation(body, player))
            .filter(|player| !body_entity.has_passenger(player.entity_id()))
            .min_by(|a, b| {
                let a = a.get_entity().pos.load().squared_distance_to_vec(&body_pos);
                let b = b.get_entity().pos.load().squared_distance_to_vec(&body_pos);
                a.total_cmp(&b)
            })
            .map(Arc::clone);

        tick.brain.set_optional(types::TEMPTING_PLAYER, player);
    }
}
