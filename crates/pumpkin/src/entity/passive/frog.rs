use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::optional_int::OptionalInt;
use pumpkin_protocol::codec::var_int::VarInt;

use crate::entity::ai::brain::behavior::utils::is_alive;
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::passive::animal::finalize_spawn_child_from_breeding;
use crate::entity::passive::frog_ai;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

const WATER_PATHFINDING_MALUS: f32 = 4.0;
const TRAPDOOR_PATHFINDING_MALUS: f32 = -1.0;
const NO_TONGUE_TARGET: i32 = -1;

use pumpkin_data::frog_variant::FrogVariant;

pub const FROG_FOOD: &[&Item] = &[&Item::SLIME_BALL];

/// Represents a Frog, an amphibious mob that can eat small slimes and magma cubes.
///
/// Wiki: <https://minecraft.wiki/w/Frog>
pub struct FrogEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub tongue_target_id: AtomicI32,
}

impl FrogEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let frog = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(FrogVariant::Temperate.id() as i32),
            tongue_target_id: AtomicI32::new(NO_TONGUE_TARGET),
        });
        let mut navigator = frog
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_pathfinding_malus(PathType::Water, WATER_PATHFINDING_MALUS);
        navigator.set_pathfinding_malus(PathType::Trapdoor, TRAPDOOR_PATHFINDING_MALUS);
        drop(navigator);
        frog.mob_entity.init_brain(frog.as_ref());
        // Vanilla seeds this in `finalizeSpawn` and for bred offspring.
        frog.mob_entity.with_brain(frog.as_ref(), |tick| {
            frog_ai::init_memories(tick.brain, tick.mob);
        });
        frog
    }

    pub fn set_tongue_target(&self, target: &dyn EntityBase) {
        let id = target.get_entity().entity_id;
        if self.tongue_target_id.swap(id, Ordering::Relaxed) != id {
            self.get_entity().set_synced_data(
                pumpkin_data::tracked_data::frog::DATA_TONGUE_TARGET_ID,
                OptionalInt(Some(id)),
            );
        }
    }

    pub fn erase_tongue_target(&self) {
        if self
            .tongue_target_id
            .swap(NO_TONGUE_TARGET, Ordering::Relaxed)
            != NO_TONGUE_TARGET
        {
            self.get_entity().set_synced_data(
                pumpkin_data::tracked_data::frog::DATA_TONGUE_TARGET_ID,
                OptionalInt(None),
            );
        }
    }

    #[must_use]
    pub fn get_tongue_target(&self, world: &World) -> Option<Arc<dyn EntityBase>> {
        let id = self.tongue_target_id.load(Ordering::Relaxed);
        (id != NO_TONGUE_TARGET)
            .then(|| world.get_entity_by_id(id))
            .flatten()
    }

    #[must_use]
    pub fn get_variant(&self) -> FrogVariant {
        FrogVariant::from_id(self.variant.load(Ordering::Relaxed) as u32)
    }

    pub fn set_variant(&self, variant: FrogVariant) {
        self.variant.store(variant.id() as i32, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::frog::VARIANT,
            VarInt(variant.id() as i32),
        );
    }
}

impl AgeableMob for FrogEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for FrogEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
            || FROG_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }

    /// Vanilla `Frog.spawnChildFromBreeding`: no offspring, the frog carries spawn instead.
    fn spawn_child_from_breeding(&self, tick: &mut BrainTick<'_>, mate: &dyn EntityBase) {
        finalize_spawn_child_from_breeding(self, mate);
        tick.brain.set(types::IS_PREGNANT, ());
    }
}

impl Mob for FrogEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string("variant", self.get_variant().asset_id().to_string());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant_str) = nbt.get_string("variant") {
            self.set_variant(FrogVariant::from_name(variant_str).unwrap_or_default());
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        frog_ai::FROG_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        frog_ai::update_activity(tick);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);
    }

    fn mob_set_variant_name(&self, name: &str) {
        self.set_variant(FrogVariant::from_name(name).unwrap_or_default());
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::frog::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::frog::VARIANT,
            VarInt(self.get_variant().id() as i32),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityFrogAmbient)
    }
}
