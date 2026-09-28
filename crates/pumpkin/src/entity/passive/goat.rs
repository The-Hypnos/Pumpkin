use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::brain::behavior::utils::is_alive;
use crate::entity::ai::brain::memory::PackedMemories;
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::item::ItemEntity;
use crate::entity::passive::goat_ai;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[&Item::WHEAT];
const ADULT_ATTACK_DAMAGE: f64 = 2.0;
const BABY_ATTACK_DAMAGE: f64 = 1.0;
// Vanilla builds this `ItemEntity` directly, which leaves the pickup delay at 0.
const HORN_DROP_PICKUP_DELAY: u8 = 0;

pub struct GoatEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub is_screaming: AtomicBool,
    pub has_left_horn: AtomicBool,
    pub has_right_horn: AtomicBool,
    was_baby: AtomicBool,
}

impl GoatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let goat = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            is_screaming: AtomicBool::new(false),
            has_left_horn: AtomicBool::new(true),
            has_right_horn: AtomicBool::new(true),
            was_baby: AtomicBool::new(false),
        });
        goat.mob_entity.init_brain(goat.as_ref());
        // Vanilla seeds these in `finalizeSpawn` and for bred offspring.
        goat.mob_entity.with_brain(goat.as_ref(), |tick| {
            goat_ai::init_memories(tick.brain, tick.mob);
        });
        goat
    }

    /// Vanilla `Goat.ageBoundaryReached`.
    fn age_boundary_reached(&self, baby: bool) {
        self.mob_entity.living_entity.set_attribute_base(
            &Attributes::ATTACK_DAMAGE,
            if baby {
                BABY_ATTACK_DAMAGE
            } else {
                ADULT_ATTACK_DAMAGE
            },
        );
    }

    /// Vanilla `Goat.dropHorn`, which picks a random side when both horns are left.
    pub fn drop_horn(&self) -> bool {
        if self.is_baby() {
            return false;
        }
        let (has_left, has_right) = (self.has_left_horn(), self.has_right_horn());
        if !has_left && !has_right {
            return false;
        }
        let mut rng = self.get_random();
        let drop_left = if !has_left {
            false
        } else if !has_right {
            true
        } else {
            rng.random::<bool>()
        };
        if drop_left {
            self.set_has_left_horn(false);
        } else {
            self.set_has_right_horn(false);
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        let velocity = Vector3::new(
            f64::from(rng.random_range(-0.2f32..0.2)),
            f64::from(rng.random_range(0.3f32..0.7)),
            f64::from(rng.random_range(-0.2f32..0.2)),
        );
        // TODO: pick the instrument from the goat horn instrument tags once they are extracted.
        let horn = ItemStack::new(1, &Item::GOAT_HORN);
        let item_entity = Entity::new(Arc::clone(&world), pos, &EntityType::ITEM);
        world.spawn_entity(Arc::new(ItemEntity::new_with_velocity(
            item_entity,
            horn,
            velocity,
            HORN_DROP_PICKUP_DELAY,
        )));
        true
    }

    #[must_use]
    pub fn is_screaming(&self) -> bool {
        self.is_screaming.load(Ordering::Relaxed)
    }

    pub fn set_screaming(&self, screaming: bool) {
        self.is_screaming.store(screaming, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_IS_SCREAMING_GOAT,
            screaming,
        );
    }

    #[must_use]
    pub fn has_left_horn(&self) -> bool {
        self.has_left_horn.load(Ordering::Relaxed)
    }

    pub fn set_has_left_horn(&self, has_horn: bool) {
        self.has_left_horn.store(has_horn, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_LEFT_HORN,
            has_horn,
        );
    }

    #[must_use]
    pub fn has_right_horn(&self) -> bool {
        self.has_right_horn.load(Ordering::Relaxed)
    }

    pub fn set_has_right_horn(&self, has_horn: bool) {
        self.has_right_horn.store(has_horn, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_RIGHT_HORN,
            has_horn,
        );
    }
}

impl AgeableMob for GoatEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for GoatEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_GOAT_FOOD)
            || TEMPT_ITEMS.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for GoatEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsScreamingGoat", self.is_screaming());
        nbt.put_bool("HasLeftHorn", self.has_left_horn());
        nbt.put_bool("HasRightHorn", self.has_right_horn());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(screaming) = nbt.get_bool("IsScreamingGoat") {
            self.set_screaming(screaming);
        }
        if let Some(left) = nbt.get_bool("HasLeftHorn") {
            self.set_has_left_horn(left);
        }
        if let Some(right) = nbt.get_bool("HasRightHorn") {
            self.set_has_right_horn(right);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        goat_ai::GOAT_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        goat_ai::update_activity(tick);
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
        let baby = self.is_baby();
        if self.was_baby.swap(baby, Ordering::Relaxed) != baby {
            self.age_boundary_reached(baby);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::goat::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_IS_SCREAMING_GOAT,
            self.is_screaming(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_LEFT_HORN,
            self.has_left_horn(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::goat::DATA_HAS_RIGHT_HORN,
            self.has_right_horn(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();
        if item == &Item::BUCKET && !self.is_baby() {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let entity = self.get_entity();
            let world = entity.world.load();
            let sound = if self.is_screaming() {
                Sound::EntityGoatScreamingMilk
            } else {
                Sound::EntityGoatMilk
            };
            world.play_sound(sound, SoundCategory::Neutral, &entity.pos.load());
            return true;
        }

        let ambient_sound = if self.is_screaming() {
            Sound::EntityGoatScreamingAmbient
        } else {
            Sound::EntityGoatAmbient
        };
        self.animal_interact(player, item_stack, ambient_sound)
    }
}
