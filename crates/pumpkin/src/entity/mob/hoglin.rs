use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, damage::DamageType};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;

use crate::entity::ageable::{AgeableData, AgeableMob};
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::mob::hoglin_base::{self, ATTACK_ANIMATION_DURATION};
use crate::entity::mob::sounds::{self, AmbientSoundTimer};
use crate::entity::mob::{Mob, MobEntity, hoglin_ai};
use crate::entity::passive::animal::Animal;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

pub struct HoglinEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
    pub cannot_be_hunted: AtomicBool,
    attack_animation_remaining_ticks: AtomicI32,
    ambient_sound_timer: AmbientSoundTimer,
    was_baby: AtomicBool,
    /// Copy of the `NEAREST_REPELLENT` memory for `get_walk_target_value`, which runs inside
    /// the brain tick where the brain cannot be locked again.
    nearest_repellent: AtomicCell<Option<BlockPos>>,
}

impl HoglinEntity {
    pub const CONVERSION_TIME: i32 = 300;
    const ADULT_XP_REWARD: u32 = 5;
    const BABY_XP_REWARD: u32 = 3;
    const ADULT_ATTACK_DAMAGE: f64 = 6.0;
    const BABY_ATTACK_DAMAGE: f64 = 0.5;
    pub const BABY_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.75,
        height: 0.85,
        eye_height: 0.625,
    };

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        {
            let mut attributes = mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = 40.0;
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = 0.3;
                speed.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(knockback_res) = attributes.get_mut(&Attributes::KNOCKBACK_RESISTANCE.id) {
                knockback_res.base_value = 0.6;
                knockback_res.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(attack_kb) = attributes.get_mut(&Attributes::ATTACK_KNOCKBACK.id) {
                attack_kb.base_value = 1.0;
                attack_kb.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(damage) = attributes.get_mut(&Attributes::ATTACK_DAMAGE.id) {
                damage.base_value = Self::ADULT_ATTACK_DAMAGE;
                damage.dirty.store(true, Ordering::Relaxed);
            }
        }
        mob_entity.living_entity.health.store(40.0);

        let hoglin = Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            immune_to_zombification: AtomicBool::new(false),
            time_in_overworld: AtomicI32::new(0),
            cannot_be_hunted: AtomicBool::new(false),
            attack_animation_remaining_ticks: AtomicI32::new(0),
            ambient_sound_timer: AmbientSoundTimer::new(),
            was_baby: AtomicBool::new(false),
            nearest_repellent: AtomicCell::new(None),
        });
        hoglin.mob_entity.init_brain(hoglin.as_ref());
        hoglin
    }

    #[must_use]
    pub fn is_adult(&self) -> bool {
        !self.is_baby()
    }

    #[must_use]
    pub fn can_be_hunted(&self) -> bool {
        self.is_adult() && !self.cannot_be_hunted.load(Ordering::Relaxed)
    }

    pub fn is_immune_to_zombification(&self) -> bool {
        self.immune_to_zombification.load(Ordering::Relaxed)
    }

    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.immune_to_zombification
            .store(immune, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::hoglin::DATA_IMMUNE_TO_ZOMBIFICATION,
            immune,
        );
    }

    pub fn is_converting(&self, world: &World) -> bool {
        !self.is_immune_to_zombification()
            && !self.mob_entity.is_no_ai()
            && world.dimension.piglins_zombify
    }

    pub fn make_sound(&self, sound: Sound) {
        sounds::make_sound(self, sound, SoundCategory::Hostile);
    }

    fn tick_ambient_sound(&self) {
        if self.ambient_sound_timer.tick() {
            let sound = self
                .mob_entity
                .with_brain(self, |tick| hoglin_ai::get_sound_for_current_activity(tick));
            if let Some(sound) = sound {
                self.make_sound(sound);
            }
        }
    }

    /// Vanilla `Hoglin.ageBoundaryReached`. Pumpkin writes the age from several places, so
    /// the tick checks for the crossing instead of `set_age`.
    fn age_boundary_reached(&self, baby: bool) {
        let living = &self.mob_entity.living_entity;
        if baby {
            living.set_attribute_base(&Attributes::ATTACK_DAMAGE, Self::BABY_ATTACK_DAMAGE);
            living.entity.entity_dimension.store(Self::BABY_DIMENSIONS);
        } else {
            living.set_attribute_base(&Attributes::ATTACK_DAMAGE, Self::ADULT_ATTACK_DAMAGE);
            let entity_type = &EntityType::HOGLIN;
            living.entity.entity_dimension.store(EntityDimensions {
                width: entity_type.dimension[0],
                height: entity_type.dimension[1],
                eye_height: entity_type.eye_height,
            });
        }
    }

    fn convert_to_zoglin(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        if world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful {
            world.play_sound(
                Sound::EntityHoglinConvertedToZombified,
                SoundCategory::Hostile,
                &pos,
            );
        }

        let zoglin = crate::entity::r#type::from_type(
            &EntityType::ZOGLIN,
            pos,
            &world,
            uuid::Uuid::new_v4(),
        );

        let zoglin_base = zoglin.get_entity();
        zoglin_base.set_rotation(entity.yaw.load(), entity.pitch.load());
        zoglin_base.head_yaw.store(entity.head_yaw.load());
        zoglin_base.velocity.store(entity.velocity.load());

        if let Some(living) = zoglin.get_living_entity() {
            living.set_health(self.mob_entity.living_entity.health.load());
        }

        if let Some(custom_name) = &**entity.custom_name.load() {
            zoglin_base.set_custom_name(custom_name.clone());
        }

        // Vanilla ConversionType.SINGLE keeps the baby state.
        if self.is_baby()
            && let Some(mob) = zoglin.get_mob()
        {
            mob.spawn_as_baby();
        }

        world.spawn_entity(zoglin);
        entity.remove();
    }
}

impl AgeableMob for HoglinEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for HoglinEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HOGLIN_FOOD)
    }
}

impl Mob for HoglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        hoglin_ai::HOGLIN_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        hoglin_ai::update_activity(tick);
        self.nearest_repellent
            .store(tick.brain.get(types::NEAREST_REPELLENT).copied());
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !crate::entity::ai::brain::behavior::utils::is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);

        let world = self.mob_entity.living_entity.entity.world.load();
        if self.is_converting(&world) {
            let time = self.time_in_overworld.fetch_add(1, Ordering::Relaxed) + 1;
            if time > Self::CONVERSION_TIME {
                self.convert_to_zoglin();
            }
        } else {
            self.time_in_overworld.store(0, Ordering::Relaxed);
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if !crate::entity::ai::brain::behavior::utils::is_alive(self) {
            return;
        }
        if self
            .attack_animation_remaining_ticks
            .load(Ordering::Relaxed)
            > 0
        {
            self.attack_animation_remaining_ticks
                .fetch_sub(1, Ordering::Relaxed);
        }
        self.ageable_ai_step();
        let baby = self.is_baby();
        if self.was_baby.swap(baby, Ordering::Relaxed) != baby {
            self.age_boundary_reached(baby);
        }
        self.tick_ambient_sound();
    }

    fn get_walk_target_value(&self, pos: &BlockPos) -> f32 {
        if hoglin_ai::is_pos_near_nearest_repellent(self.nearest_repellent.load(), pos) {
            return -1.0;
        }
        let world = self.mob_entity.living_entity.entity.world.load();
        if world.get_block(&pos.down()).id == Block::CRIMSON_NYLIUM.id {
            10.0
        } else {
            0.0
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // Vanilla `canFallInLove` refuses while pacified, so the food is not taken.
        let pacified = self.is_food(item_stack)
            && self.is_adult()
            && self
                .mob_entity
                .with_brain(self, |tick| hoglin_ai::is_pacified(tick.brain));
        let consumed = if pacified {
            self.mob_entity.mob_interact(player, item_stack)
        } else {
            self.animal_interact(player, item_stack, Sound::EntityHoglinAmbient)
        };
        if consumed {
            self.mob_entity
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
        consumed
    }

    fn do_hurt_target(&self, target: &dyn EntityBase) {
        if target.get_living_entity().is_none() {
            return;
        }
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        self.attack_animation_remaining_ticks
            .store(ATTACK_ANIMATION_DURATION, Ordering::Relaxed);
        world.send_entity_status(entity, EntityStatus::StartAttacking, None);
        self.make_sound(Sound::EntityHoglinAttack);
        if let Some(hit) = world.get_entity_by_id(target.get_entity().entity_id) {
            // Queued: this runs inside our own brain tick
            self.mob_entity.post_to_brain(Box::new(move |tick| {
                hoglin_ai::on_hit_target(tick, &hit);
            }));
        }
        hoglin_base::hurt_and_throw_target(self, target);
    }

    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        self.ambient_sound_timer.reset();
        let Some(attacker) = source else {
            return;
        };
        let world = self.mob_entity.living_entity.entity.world.load_full();
        let Some(attacker) = world
            .get_entity_by_id(attacker.get_entity().entity_id)
            .filter(|attacker| attacker.get_living_entity().is_some())
        else {
            return;
        };
        // Queued: taking the victim's brain here deadlocks two mobs fighting
        self.mob_entity.post_to_brain(Box::new(move |tick| {
            hoglin_ai::was_hurt_by(tick, &attacker);
        }));
    }

    fn get_base_experience_reward(&self) -> u32 {
        if self.is_baby() {
            Self::BABY_XP_REWARD
        } else {
            Self::ADULT_XP_REWARD
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if self.is_immune_to_zombification() {
            nbt.put_bool("IsImmuneToZombification", true);
        }
        let time = self.time_in_overworld.load(Ordering::Relaxed);
        if time > 0 {
            nbt.put_int("TimeInOverworld", time);
        }
        if self.cannot_be_hunted.load(Ordering::Relaxed) {
            nbt.put_bool("CannotBeHunted", true);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(immune) = nbt.get_bool("IsImmuneToZombification") {
            self.set_immune_to_zombification(immune);
        }
        if let Some(time) = nbt.get_int("TimeInOverworld") {
            self.time_in_overworld.store(time, Ordering::Relaxed);
        }
        if let Some(cannot_hunt) = nbt.get_bool("CannotBeHunted") {
            self.cannot_be_hunted.store(cannot_hunt, Ordering::Relaxed);
        }
    }
}

/// Vanilla `HoglinBase.throwTarget`, shared with zoglins.
pub fn throw_target(attacker: &Entity, target: &dyn EntityBase) {
    let my_pos = attacker.pos.load();
    let target_pos = target.get_entity().pos.load();
    let dx = target_pos.x - my_pos.x;
    let dz = target_pos.z - my_pos.z;
    let dist = dx.hypot(dz).max(0.001);
    let vel = target.get_entity().velocity.load();
    target
        .get_entity()
        .velocity
        .store(pumpkin_util::math::vector3::Vector3::new(
            vel.x + (dx / dist) * 0.5,
            0.5,
            vel.z + (dz / dist) * 0.5,
        ));
}
