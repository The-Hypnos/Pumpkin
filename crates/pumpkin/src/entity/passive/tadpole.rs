use std::sync::Arc;

use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::brain::behavior::utils::is_alive;
use crate::entity::ai::brain::memory::PackedMemories;
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::passive::tadpole_ai;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    player::Player,
};

pub struct TadpoleEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
}

impl TadpoleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let tadpole = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
        });
        tadpole.mob_entity.init_brain(tadpole.as_ref());
        tadpole
    }

    fn is_food(item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
            || item_stack.item == &Item::SLIME_BALL
    }
}

impl AgeableMob for TadpoleEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Mob for TadpoleEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        tadpole_ai::TADPOLE_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        tadpole_ai::update_activity(tick);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if Self::is_food(item_stack) {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let age = self
                .get_mob_entity()
                .living_entity
                .entity
                .age
                .load(std::sync::atomic::Ordering::Relaxed);
            let speedup = (-age / 10).max(1);
            self.get_mob_entity()
                .living_entity
                .entity
                .age
                .fetch_add(speedup, std::sync::atomic::Ordering::Relaxed);

            let entity = self.get_entity();
            let world = entity.world.load();
            let pos = entity.pos.load();
            world.spawn_particle(
                pos + Vector3::new(0.0, f64::from(entity.height()), 0.0),
                Vector3::new(0.5, 0.5, 0.5),
                1.0,
                7,
                Particle::HappyVillager,
            );
            world.play_sound(Sound::EntityTadpoleGrowUp, SoundCategory::Neutral, &pos);
            return true;
        }

        if item_stack.get_item() == &Item::WATER_BUCKET {
            let entity = self.get_entity();
            let world = entity.world.load();
            if let Some(server) = world.server.upgrade() {
                let mut event = crate::plugin::api::events::player::player_bucket_entity::PlayerBucketEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    bucket_item: "tadpole_bucket".to_string(),
                    cancelled: false,
                };
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return false;
                }
            }
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let pos = entity.pos.load();
            world.play_sound(Sound::ItemBucketFillTadpole, SoundCategory::Neutral, &pos);
            entity.remove();
            return true;
        }

        false
    }
}
