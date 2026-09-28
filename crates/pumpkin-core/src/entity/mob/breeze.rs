use std::sync::Arc;

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;

use crate::entity::ai::brain::behavior::utils::is_alive;
use crate::entity::ai::brain::memory::PackedMemories;
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::mob::breeze_ai;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};

pub struct BreezeEntity {
    pub mob_entity: MobEntity,
}

impl BreezeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let breeze = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
        });
        breeze.mob_entity.init_brain(breeze.as_ref());
        breeze
    }
}

impl Mob for BreezeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        let mut brain = breeze_ai::BREEZE_PROVIDER.make_brain(self, packed);
        brain.set_default_activity(Activity::Fight);
        brain.use_default_activity();
        brain
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        breeze_ai::update_activity(tick);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);
    }

    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        let target_type = target.get_entity().entity_type;
        (target_type == &EntityType::PLAYER || target_type == &EntityType::IRON_GOLEM)
            && self.mob_entity.living_entity.can_attack(target)
    }

    /// Vanilla `Breeze.isInvulnerableTo`: breezes can't hurt each other.
    fn pre_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        // Only breezes fire breeze wind charges, so the charge stands in for its owner here.
        !source.is_some_and(|source| {
            let source_type = source.get_entity().entity_type;
            source_type == &EntityType::BREEZE || source_type == &EntityType::BREEZE_WIND_CHARGE
        })
    }
}
