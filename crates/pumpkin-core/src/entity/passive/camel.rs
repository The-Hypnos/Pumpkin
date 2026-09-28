use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering},
};

use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};

use crate::entity::ai::brain::behavior::utils::is_alive;
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::mob::sounds;
use crate::entity::passive::camel_ai;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

pub const FLAG_TAME: u8 = 2;
pub const FLAG_SADDLE: u8 = 4;
pub const FLAG_BRED: u8 = 8;
pub const FLAG_EATING: u8 = 16;
pub const FLAG_STANDING: u8 = 32;

const SITDOWN_DURATION_TICKS: i64 = 40;
const STANDUP_DURATION_TICKS: i64 = 52;
const ADULT_SITTING_HEIGHT_REDUCTION: f32 = 1.43;
const ADULT_SITTING_EYE_HEIGHT: f32 = 0.845;
const BABY_STANDING_DIMENSIONS: EntityDimensions = EntityDimensions {
    width: 0.95,
    height: 1.4,
    eye_height: 1.38,
};
const BABY_SITTING_DIMENSIONS: EntityDimensions = EntityDimensions {
    width: 0.95,
    height: 0.425,
    eye_height: 0.41,
};

pub struct CamelEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub flags: AtomicU8,
    pub dashing: AtomicBool,
    /// Vanilla `LAST_POSE_CHANGE_TICK`: negative while sitting, its magnitude the change time.
    last_pose_change_tick: AtomicI64,
}

impl CamelEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let camel = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            flags: AtomicU8::new(0),
            dashing: AtomicBool::new(false),
            last_pose_change_tick: AtomicI64::new(0),
        });
        // Vanilla does this in `finalizeSpawn`; a saved camel overwrites it from NBT.
        let now = camel.get_entity().world.load().get_world_age();
        camel.reset_last_pose_change_tick_to_full_stand(now);
        camel.mob_entity.init_brain(camel.as_ref());
        camel
    }

    #[must_use]
    pub fn is_camel_sitting(&self) -> bool {
        self.last_pose_change_tick.load(Ordering::Relaxed) < 0
    }

    #[must_use]
    pub fn get_pose_time(&self, time: i64) -> i64 {
        time - self.last_pose_change_tick.load(Ordering::Relaxed).abs()
    }

    #[must_use]
    pub fn is_in_pose_transition(&self, time: i64) -> bool {
        let duration = if self.is_camel_sitting() {
            SITDOWN_DURATION_TICKS
        } else {
            STANDUP_DURATION_TICKS
        };
        self.get_pose_time(time) < duration
    }

    #[must_use]
    pub fn refuse_to_move(&self, time: i64) -> bool {
        self.is_camel_sitting() || self.is_in_pose_transition(time)
    }

    /// Vanilla `canCamelChangePose`: the other pose's hitbox must not collide.
    #[must_use]
    pub fn can_camel_change_pose(&self, world: &World) -> bool {
        let dimensions = self.dimensions_for(!self.is_camel_sitting());
        let pos = self.get_entity().pos.load();
        world.is_space_empty(BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions))
    }

    pub fn sit_down(&self, time: i64) {
        if self.is_camel_sitting() {
            return;
        }
        sounds::make_sound(self, Sound::EntityCamelSit, SoundCategory::Neutral);
        self.set_camel_pose(EntityPose::Sitting);
        self.reset_last_pose_change_tick(-time);
    }

    pub fn stand_up(&self, time: i64) {
        if !self.is_camel_sitting() {
            return;
        }
        sounds::make_sound(self, Sound::EntityCamelStand, SoundCategory::Neutral);
        self.set_camel_pose(EntityPose::Standing);
        self.reset_last_pose_change_tick(time);
    }

    pub fn stand_up_instantly(&self, time: i64) {
        self.set_camel_pose(EntityPose::Standing);
        self.reset_last_pose_change_tick_to_full_stand(time);
    }

    fn set_camel_pose(&self, pose: EntityPose) {
        let entity = self.get_entity();
        entity.set_pose(pose);
        entity
            .entity_dimension
            .store(self.dimensions_for(pose == EntityPose::Sitting));
        entity
            .world
            .load()
            .emit_game_event("entity_action", entity.pos.load());
    }

    /// Vanilla `Camel.getDefaultDimensions`.
    fn dimensions_for(&self, sitting: bool) -> EntityDimensions {
        let camel = &EntityType::CAMEL;
        match (sitting, self.is_baby()) {
            (true, true) => BABY_SITTING_DIMENSIONS,
            (true, false) => EntityDimensions {
                width: camel.dimension[0],
                height: camel.dimension[1] - ADULT_SITTING_HEIGHT_REDUCTION,
                eye_height: ADULT_SITTING_EYE_HEIGHT,
            },
            (false, true) => BABY_STANDING_DIMENSIONS,
            (false, false) => EntityDimensions {
                width: camel.dimension[0],
                height: camel.dimension[1],
                eye_height: camel.eye_height,
            },
        }
    }

    fn reset_last_pose_change_tick(&self, synced_pose_tick_time: i64) {
        self.last_pose_change_tick
            .store(synced_pose_tick_time, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::camel::LAST_POSE_CHANGE_TICK,
            synced_pose_tick_time,
        );
    }

    fn reset_last_pose_change_tick_to_full_stand(&self, time: i64) {
        self.reset_last_pose_change_tick((time - STANDUP_DURATION_TICKS - 1).max(0));
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::camel::DATA_ID_FLAGS,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn is_saddled(&self) -> bool {
        self.has_flag(FLAG_SADDLE)
    }

    pub fn set_saddled(&self, val: bool) {
        self.set_flag(FLAG_SADDLE, val);
    }

    #[must_use]
    pub fn is_dashing(&self) -> bool {
        self.dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        self.dashing.store(dashing, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::camel::DASH, dashing);
    }
}

impl AgeableMob for CamelEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for CamelEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_CAMEL_FOOD)
            || item_stack.item == &Item::CACTUS
    }
}

impl Mob for CamelEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_bool("Saddle", self.is_saddled());
        nbt.put_long(
            "LastPoseTick",
            self.last_pose_change_tick.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(saddle) = nbt.get_bool("Saddle") {
            self.set_saddled(saddle);
        }
        let pose_tick = nbt.get_long("LastPoseTick").unwrap_or(0);
        if pose_tick < 0 {
            self.set_camel_pose(EntityPose::Sitting);
        }
        self.reset_last_pose_change_tick(pose_tick);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        camel_ai::CAMEL_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        camel_ai::update_activity(tick);
        // Vanilla `CamelMoveControl`: a sitting camel told to walk stands up first.
        if tick.brain.has_memory_value(types::WALK_TARGET.id())
            && !self.get_entity().is_leashed()
            && self.is_camel_sitting()
            && !self.is_in_pose_transition(tick.time)
            && self.can_camel_change_pose(tick.world)
        {
            self.stand_up(tick.time);
        }
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
        let entity = self.get_entity();
        let time = entity.world.load().get_world_age();
        if self.is_camel_sitting() && entity.is_in_water() {
            self.stand_up_instantly(time);
        }
        // Vanilla `Camel.travel`: no horizontal movement while sitting or changing pose.
        if self.refuse_to_move(time) && entity.on_ground.load(Ordering::Relaxed) {
            entity
                .velocity
                .store(entity.velocity.load().multiply(0.0, 1.0, 0.0));
            let living = &self.mob_entity.living_entity;
            living
                .movement_input
                .store(living.movement_input.load().multiply(0.0, 1.0, 0.0));
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::camel::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::camel::DATA_ID_FLAGS,
            self.flags.load(Ordering::Relaxed) as i8,
        );
        entity.set_synced_data(pumpkin_data::tracked_data::camel::DASH, self.is_dashing());
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();

        if item == &Item::SADDLE && !self.is_saddled() && !self.is_baby() {
            self.set_saddled(true);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let entity = self.get_entity();
            let world = entity.world.load();
            world.play_sound(
                Sound::EntityCamelSaddle,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }

        if self.is_saddled() && !self.is_baby() && !self.is_food(item_stack) {
            let world = player.world();
            let ent = &self.mob_entity.living_entity.entity;
            if let Some(vehicle) = world.get_entity_by_id(ent.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                ent.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                return true;
            }
        }

        self.animal_interact(player, item_stack, Sound::EntityCamelAmbient)
    }
}
