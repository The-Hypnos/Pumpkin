use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{EquipmentSlot, PotionContentsImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use crate::entity::ai::brain::behavior::utils::{is_alive, throw_item};
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::item::ItemEntity;
use crate::entity::passive::allay_ai;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
    player::Player,
};

const HEAL_INTERVAL: i32 = 10;
const ITEM_GIVEN_VOLUME: f32 = 2.0;

pub struct AllayEntity {
    pub mob_entity: MobEntity,
    pub dancing: AtomicBool,
    pub can_duplicate: AtomicBool,
    pub duplication_cooldown: AtomicI32,
    /// Vanilla's one-slot `SimpleContainer` of items collected to deliver.
    inventory: Mutex<ItemStack>,
    /// Lock-free copy of the `LIKED_PLAYER` memory, for damage checks from other entities' ticks.
    liked_player: AtomicCell<Option<Uuid>>,
    tick_count: AtomicI32,
}

impl AllayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let allay = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            dancing: AtomicBool::new(false),
            can_duplicate: AtomicBool::new(true),
            duplication_cooldown: AtomicI32::new(0),
            inventory: Mutex::new(ItemStack::EMPTY.clone()),
            liked_player: AtomicCell::new(None),
            tick_count: AtomicI32::new(0),
        });
        allay.mob_entity.init_brain(allay.as_ref());
        allay
    }

    #[must_use]
    pub fn is_dancing(&self) -> bool {
        self.dancing.load(Ordering::Relaxed)
    }

    pub fn set_dancing(&self, dancing: bool) {
        self.dancing.store(dancing, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::allay::DATA_DANCING, dancing);
    }

    #[must_use]
    pub fn can_duplicate(&self) -> bool {
        self.can_duplicate.load(Ordering::Relaxed)
    }

    pub fn set_can_duplicate(&self, can_duplicate: bool) {
        self.can_duplicate.store(can_duplicate, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::allay::DATA_CAN_DUPLICATE,
            can_duplicate,
        );
    }

    fn item_in_hand(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    /// Vanilla `Allay.hasItemInHand`.
    #[must_use]
    pub fn has_item_in_hand(&self) -> bool {
        !self.item_in_hand().is_empty()
    }

    #[must_use]
    pub fn inventory_item(&self) -> ItemStack {
        self.inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Vanilla `SimpleContainer.removeItem(0, count)`.
    pub fn remove_inventory_item(&self, count: u8) -> ItemStack {
        self.inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .split(count)
    }

    fn take_inventory(&self) -> ItemStack {
        std::mem::replace(
            &mut *self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ItemStack::EMPTY.clone(),
        )
    }

    /// Vanilla `SimpleContainer.canAddItem` for the single slot.
    fn can_add_to_inventory(&self, stack: &ItemStack) -> bool {
        let slot = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.is_empty()
            || (slot.are_items_and_components_equal(stack)
                && slot.item_count < slot.get_max_stack_size())
    }

    /// Vanilla `Allay.allayConsidersItemEqual`: same item, and the same potion if either has one.
    fn considers_item_equal(first: &ItemStack, second: &ItemStack) -> bool {
        first.item.id == second.item.id
            && first.get_data_component::<PotionContentsImpl>()
                == second.get_data_component::<PotionContentsImpl>()
    }

    fn is_liked_player(&self, entity: &dyn EntityBase) -> bool {
        entity
            .get_player()
            .is_some_and(|player| self.liked_player.load() == Some(player.gameprofile.id))
    }

    fn drop_inventory(&self) {
        let stack = self.take_inventory();
        if stack.is_empty() {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let item = ItemEntity::new(
            Entity::new(Arc::clone(&world), entity.pos.load(), &EntityType::ITEM),
            stack,
        );
        world.spawn_entity(Arc::new(item));
    }

    /// Vanilla `Mob.aiStep` item pickup, with the allay's reach of one block on every axis.
    fn pick_up_nearby_items(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        // Cheap checks first: most allays hold nothing, and then never need the brain.
        if !self.has_item_in_hand()
            || !is_alive(self)
            || !world.level_info.load().game_rules.mob_griefing
        {
            return;
        }
        let reach = entity.bounding_box.load().expand(1.0, 1.0, 1.0);
        let candidates = world.get_entities_at_box(&reach);
        if candidates.is_empty() {
            return;
        }
        self.mob_entity.with_brain(self, |tick| {
            if !self.can_pick_up_loot(tick.brain) {
                return;
            }
            for candidate in &candidates {
                let Some(item) = candidate.get_item_entity() else {
                    continue;
                };
                if item.get_entity().is_alive() && item.get_pickup_delay() == 0 {
                    self.pick_up_item(tick.brain, item);
                }
            }
        });
    }

    /// Vanilla `InventoryCarrier.pickUpItem`.
    fn pick_up_item(&self, brain: &Brain, item: &ItemEntity) {
        let stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if stack.is_empty() || !self.wants_to_pick_up(brain, &stack) {
            return;
        }
        let space = {
            let slot = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_empty() {
                stack.get_max_stack_size()
            } else {
                slot.get_max_stack_size().saturating_sub(slot.item_count)
            }
        };
        let taken = stack.item_count.min(space);
        if taken == 0
            || !self
                .mob_entity
                .living_entity
                .pickup(item.get_entity(), u32::from(taken))
        {
            return;
        }
        let moved = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .split(taken);
        {
            let mut slot = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_empty() {
                *slot = moved;
            } else {
                slot.increment(moved.item_count);
            }
        }
        let emptied = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if emptied {
            item.get_entity().remove();
        } else {
            item.init_data_tracker();
        }
    }

    fn update_duplication_cooldown(&self) {
        let cooldown = self.duplication_cooldown.load(Ordering::Relaxed);
        if cooldown > 0 {
            let next = cooldown - 1;
            self.duplication_cooldown.store(next, Ordering::Relaxed);
            if next == 0 {
                self.set_can_duplicate(true);
            }
        }
    }
}

impl Mob for AllayEntity {
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("CanDuplicate", self.can_duplicate());
        nbt.put_int(
            "DuplicationCooldown",
            self.duplication_cooldown.load(Ordering::Relaxed),
        );
        let stack = self.inventory_item();
        if !stack.is_empty() {
            let mut item_nbt = NbtCompound::new();
            stack.write_item_stack(&mut item_nbt);
            nbt.put_list("Inventory", vec![NbtTag::Compound(item_nbt)]);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(can) = nbt.get_bool("CanDuplicate") {
            self.set_can_duplicate(can);
        }
        if let Some(cd) = nbt.get_int("DuplicationCooldown") {
            self.duplication_cooldown.store(cd, Ordering::Relaxed);
        }
        if let Some(stack) = nbt
            .get_list("Inventory")
            .and_then(|list| list.first())
            .and_then(NbtTag::extract_compound)
            .and_then(ItemStack::read_item_stack)
        {
            *self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = stack;
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        allay_ai::ALLAY_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        allay_ai::update_activity(tick);
        self.liked_player
            .store(tick.brain.get(types::LIKED_PLAYER).copied());
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let tick_count = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        if is_alive(self) && tick_count % HEAL_INTERVAL == 0 {
            self.mob_entity.living_entity.heal(1.0);
        }
        self.update_duplication_cooldown();
        if self.is_dancing() && self.is_panicking() {
            self.set_dancing(false);
        }
    }

    fn post_tick(&self) {
        self.pick_up_nearby_items();
    }

    /// Vanilla `Allay.canPickUpLoot`.
    fn can_pick_up_loot(&self, brain: &Brain) -> bool {
        !brain.has_memory_value(types::ITEM_PICKUP_COOLDOWN_TICKS.id()) && self.has_item_in_hand()
    }

    /// Vanilla `Allay.wantsToPickUp`.
    fn wants_to_pick_up(&self, _brain: &Brain, stack: &ItemStack) -> bool {
        let in_hand = self.item_in_hand();
        !in_hand.is_empty()
            && self
                .get_entity()
                .world
                .load()
                .level_info
                .load()
                .game_rules
                .mob_griefing
            && self.can_add_to_inventory(stack)
            && Self::considers_item_equal(&in_hand, stack)
    }

    /// Vanilla `Allay.hurtServer`: its liked player can't hurt it.
    fn pre_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        !source.is_some_and(|source| self.is_liked_player(source))
    }

    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        if self.mob_entity.living_entity.dead.load(Ordering::Relaxed) {
            self.drop_inventory();
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::allay::DATA_DANCING,
            self.is_dancing(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::allay::DATA_CAN_DUPLICATE,
            self.can_duplicate(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();

        if self.is_dancing() && self.can_duplicate() && item == &Item::AMETHYST_SHARD {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            self.set_can_duplicate(false);
            self.duplication_cooldown.store(6000, Ordering::Relaxed);

            let entity = self.get_entity();
            let world = entity.world.load();
            let pos = entity.pos.load();
            world.spawn_particle(
                pos + Vector3::new(0.0, f64::from(entity.height()), 0.0),
                Vector3::new(0.5, 0.5, 0.5),
                1.0,
                7,
                Particle::Heart,
            );
            world.play_sound(
                Sound::EntityAllayAmbientWithoutItem,
                SoundCategory::Neutral,
                &pos,
            );

            let new_allay = Self::new(Entity::new(world.clone(), pos, &EntityType::ALLAY));
            world.spawn_entity(new_allay);
            return true;
        }

        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let in_hand = self.item_in_hand();

        if in_hand.is_empty() && !item_stack.is_empty() {
            self.mob_entity.set_item_slot_and_drop_when_killed(
                &EquipmentSlot::MAIN_HAND,
                item_stack.copy_with_count(1),
            );
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            world.play_sound_raw_expect(
                player,
                Sound::EntityAllayItemGiven as u16,
                SoundCategory::Neutral,
                &pos,
                ITEM_GIVEN_VOLUME,
                1.0,
            );
            let liked = player.gameprofile.id;
            self.liked_player.store(Some(liked));
            self.mob_entity.with_brain(self, |tick| {
                tick.brain.set(types::LIKED_PLAYER, liked);
            });
            return true;
        }

        if !in_hand.is_empty() && item_stack.is_empty() {
            self.mob_entity
                .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
            world.play_sound_raw_expect(
                player,
                Sound::EntityAllayItemTaken as u16,
                SoundCategory::Neutral,
                &pos,
                ITEM_GIVEN_VOLUME,
                1.0,
            );
            self.mob_entity.living_entity.swing_hand();
            let stored = self.take_inventory();
            if !stored.is_empty() {
                throw_item(self, stored, pos, Vector3::new(0.3, 0.3, 0.3), 0.3);
            }
            self.liked_player.store(None);
            self.mob_entity.with_brain(self, |tick| {
                tick.brain.erase(types::LIKED_PLAYER.id());
            });
            let mut returned = in_hand;
            player.inventory.insert_stack_anywhere(&mut returned);
            return true;
        }

        false
    }
}
