use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::{
    Block, BlockState,
    damage::DamageType,
    data_component_impl::EquipmentSlot,
    effect::StatusEffect,
    entity::{EntityStatus, EntityType},
    game_event::GameEvent,
    item::Item,
    item_stack::ItemStack,
    potion::Effect,
    sound::Sound,
    tag::{self, Taggable},
    tracked_data::TrackedData,
};
use pumpkin_inventory::inventory::SimpleInventory;
use pumpkin_inventory::mount_screen_handler::MountScreenHandler;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ageable::{AgeableData, AgeableMob};
use crate::entity::ai::brain::{
    Brain,
    memory::{PackedMemories, types},
};
use crate::entity::ai::control::{
    smooth_swimming_look_control::SmoothSwimmingLookControl,
    smooth_swimming_move_control::SmoothSwimmingMoveControl,
};
use crate::entity::ai::pathfinder::{Navigator, node::PathType};
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::equipment::equip_on_target;
use crate::entity::mob::neutral::find_by_uuid;
use crate::entity::mob::spawn::{EntitySpawnReason, SpawnGroupData};
use crate::entity::mob::{Mob, MobEntity};
use crate::entity::passive::abstract_horse::{bedrock_equip_slot_nbt, open_mount_inventory};
use crate::entity::passive::animal::Animal;
use crate::entity::passive::tamable::{TamableAnimal, TamableData};
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::item::items::bucket::create_filled_result;
use crate::world::World;

mod ai;

const SMALL_RESTRICTION_RADIUS: i32 = 16;
const LARGE_RESTRICTION_RADIUS: i32 = 32;
const RESTRICTION_RADIUS_BUFFER: i32 = 8;
const EFFECT_DURATION: i32 = 60;
const EFFECT_REFRESH_RATE: i64 = 40;
const NAUTILUS_WATER_RESISTANCE: f64 = 0.9;
const IN_WATER_SPEED_MODIFIER: f32 = 0.011;
const DASH_COOLDOWN_TICKS: i32 = 40;
const DASH_MINIMUM_DURATION_TICKS: i32 = 5;
const NAUTILUS_TOTAL_AIR_SUPPLY: i32 = 300;
const DRY_OUT_AIR_SUPPLY: i32 = -20;
const DRY_OUT_DAMAGE: f32 = 2.0;
const TAMING_CHANCE: i32 = 3;
const FEED_HEALING_FACTOR: f32 = 2.0;
const FEED_DEFAULT_HEAL: f32 = 1.0;
pub(super) const INVENTORY_COLUMNS: i32 = 0;

pub struct NautilusEntity {
    pub mob_entity: MobEntity,
    ageable_data: AgeableData,
    tamable_data: TamableData,
    dashing: AtomicBool,
    dash_cooldown: AtomicI32,
    inventory: Arc<SimpleInventory>,
    pending_anger_target: AtomicCell<Option<Uuid>>,
}

impl NautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity
            .air_supply
            .store(NAUTILUS_TOTAL_AIR_SUPPLY, Ordering::Relaxed);
        let mut mob_entity = MobEntity::new(entity);
        *mob_entity
            .move_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Box::new(
            SmoothSwimmingMoveControl::new(85, 10, IN_WATER_SPEED_MODIFIER, 0.0, true),
        );
        *mob_entity
            .look_control
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(SmoothSwimmingLookControl::new(10));
        let navigator = mob_entity
            .navigator
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *navigator = Navigator::water_bound(false);
        navigator.set_pathfinding_malus(PathType::Water, 0.0);
        Arc::new(Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            tamable_data: TamableData::default(),
            dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            inventory: Arc::new(SimpleInventory::new(0)),
            pending_anger_target: AtomicCell::new(None),
        })
    }

    pub fn check_nautilus_spawn_rules(world: &World, pos: &BlockPos) -> bool {
        let sea_level = world.sea_level;
        let min_spawn_level = sea_level - 25;
        pos.0.y >= min_spawn_level
            && pos.0.y <= sea_level - 5
            && world
                .get_fluid(&pos.down())
                .has_tag(&tag::Fluid::MINECRAFT_WATER)
            && world.get_block(&pos.up()) == &Block::WATER
    }

    #[must_use]
    pub fn is_dashing(&self) -> bool {
        self.dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        set_abstract_nautilus_dashing(
            self.get_entity(),
            &self.dashing,
            &self.dash_cooldown,
            pumpkin_data::tracked_data::nautilus::DASH,
            dashing,
        );
    }

    #[must_use]
    pub fn can_jump(&self) -> bool {
        self.is_saddled()
    }

    pub fn handle_start_jump(&self) {
        handle_abstract_nautilus_start_jump(self, self.get_dash_sound(), |dashing| {
            self.set_dashing(dashing);
        });
    }

    pub fn open_custom_inventory_screen(&self, player: &Arc<Player>) {
        open_mount_inventory(
            self,
            player,
            &self.inventory,
            INVENTORY_COLUMNS,
            MountScreenHandler::SLOT_SADDLE,
            bedrock_equip_slot_nbt("minecraft:saddle", 0),
            |mob| mob.as_nautilus().map(|nautilus| nautilus.inventory.clone()),
        );
    }

    fn is_baby(&self) -> bool {
        self.mob_entity.living_entity.is_baby()
    }

    fn get_dash_sound(&self) -> Sound {
        if self.get_entity().is_under_water() {
            Sound::EntityNautilusDash
        } else {
            Sound::EntityNautilusDashLand
        }
    }

    fn get_dash_ready_sound(&self) -> Sound {
        if self.get_entity().is_under_water() {
            Sound::EntityNautilusDashReady
        } else {
            Sound::EntityNautilusDashReadyLand
        }
    }

    fn get_nautilus_restriction_radius(&self) -> i32 {
        if !self.is_baby() && !self.is_saddled() {
            LARGE_RESTRICTION_RADIUS
        } else {
            SMALL_RESTRICTION_RADIUS
        }
    }

    fn check_restriction(&self) {
        let entity = self.get_entity();
        if entity.is_leashed() || entity.has_passengers() || !self.is_tame() {
            return;
        }
        let radius = self.get_nautilus_restriction_radius();
        let block_pos = entity.block_pos.load();
        let mob_entity = &self.mob_entity;
        let buffered = radius + RESTRICTION_RADIUS_BUFFER;
        if !mob_entity.has_position_target()
            || mob_entity
                .position_target
                .load()
                .squared_distance(&block_pos)
                >= buffered * buffered
            || radius != mob_entity.position_target_range.load(Ordering::Relaxed)
        {
            mob_entity.set_home_to(block_pos, radius);
        }
    }

    fn handle_air_supply(&self, pre_tick_air_supply: i32) {
        let entity = self.get_entity();
        if self.mob_entity.living_entity.is_alive() && !entity.is_in_water() {
            entity.set_air_supply(pre_tick_air_supply - 1);
            if entity.air_supply.load(Ordering::Relaxed) <= DRY_OUT_AIR_SUPPLY {
                entity.set_air_supply(0);
                self.damage(self, DRY_OUT_DAMAGE, DamageType::DRY_OUT);
            }
        } else {
            entity.set_air_supply(NAUTILUS_TOTAL_AIR_SUPPLY);
        }
    }

    fn is_aggravated(&self) -> bool {
        self.mob_entity.brain_has_memory_value(types::ANGRY_AT.id())
            || self
                .mob_entity
                .brain_has_memory_value(types::ATTACK_TARGET.id())
    }

    fn set_anger_target(&self, attacker: &dyn EntityBase) {
        let uuid = attacker.get_entity().entity_uuid;
        if self
            .mob_entity
            .try_with_brain(self, |brain| {
                ai::set_anger_target(brain, self, attacker, uuid);
            })
            .is_none()
        {
            self.pending_anger_target.store(Some(uuid));
        }
    }

    fn apply_pending_anger_target(&self) {
        let Some(uuid) = self.pending_anger_target.take() else {
            return;
        };
        let world = self.get_entity().world.load();
        if let Some(attacker) = find_by_uuid(&world, uuid) {
            self.mob_entity.with_brain(self, |brain| {
                ai::set_anger_target(brain, self, attacker.as_ref(), uuid);
            });
        }
    }
}

impl AgeableMob for NautilusEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for NautilusEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        let food_tag = if !self.is_tame() && !self.is_baby() {
            &tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS
        } else {
            &tag::Item::MINECRAFT_NAUTILUS_FOOD
        };
        item_stack.get_item().has_tag(food_tag)
    }

    fn use_player_item(&self, player: &Arc<Player>, item_stack: &mut ItemStack) {
        use_nautilus_player_item(player, item_stack);
    }

    fn play_eating_sound(&self) {
        self.make_sound(if self.is_baby() {
            Sound::EntityBabyNautilusEat
        } else {
            Sound::EntityNautilusEat
        });
    }

    fn get_breed_offspring(&self, _partner: &dyn EntityBase) -> Arc<dyn EntityBase> {
        let entity = self.get_entity();
        let baby = Self::new(Entity::new(
            entity.world.load_full(),
            entity.pos.load(),
            &EntityType::NAUTILUS,
        ));
        if self.is_tame() {
            baby.set_owner(self.get_owner());
            baby.set_tame(true);
        }
        baby
    }
}

impl TamableAnimal for NautilusEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }
}

impl CustomSound for NautilusEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let under_water = self.get_entity().is_under_water();
        Some(match (self.is_baby(), under_water) {
            (true, true) => Sound::EntityBabyNautilusHurt,
            (true, false) => Sound::EntityBabyNautilusHurtLand,
            (false, true) => Sound::EntityNautilusHurt,
            (false, false) => Sound::EntityNautilusHurtLand,
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        let under_water = self.get_entity().is_under_water();
        Some(match (self.is_baby(), under_water) {
            (true, true) => Sound::EntityBabyNautilusDeath,
            (true, false) => Sound::EntityBabyNautilusDeathLand,
            (false, true) => Sound::EntityNautilusDeath,
            (false, false) => Sound::EntityNautilusDeathLand,
        })
    }
}

impl Mob for NautilusEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ai::make_brain(self, packed)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.apply_pending_anger_target();
        self.mob_entity.tick_brain(self);
        let attack_target = self.mob_entity.with_brain(self, |brain| {
            ai::update_activity(brain);
            brain.get(types::ATTACK_TARGET).cloned()
        });
        self.mob_entity.set_target(attack_target);
        self.check_restriction();
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.mob_entity
            .with_brain(self, |brain| ai::init_memories(brain, self));
        self.finalize_spawn_super(group_data)
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }

    fn as_nautilus(&self) -> Option<&NautilusEntity> {
        Some(self)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        let under_water = self.get_entity().is_under_water();
        Some(match (self.is_baby(), under_water) {
            (true, true) => Sound::EntityBabyNautilusAmbient,
            (true, false) => Sound::EntityBabyNautilusAmbientLand,
            (false, true) => Sound::EntityNautilusAmbient,
            (false, false) => Sound::EntityNautilusAmbientLand,
        })
    }

    fn mob_get_swim_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityBabyNautilusSwim
        } else {
            Sound::EntityNautilusSwim
        })
    }

    fn mob_play_step_sound(&self, _pos: &BlockPos, _state: &BlockState) {}

    fn mob_is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn get_walk_target_value(&self, _pos: &BlockPos) -> f32 {
        0.0
    }

    fn travel_in_water(&self, caller: &dyn EntityBase, _gravity: f64, _falling: bool) {
        let living = &self.mob_entity.living_entity;
        living.entity.update_velocity_from_input(
            living.movement_input.load(),
            f64::from(living.get_speed()),
        );
        living.make_move(caller);
        living
            .entity
            .velocity
            .store(living.entity.velocity.load() * NAUTILUS_WATER_RESISTANCE);
    }

    fn check_spawn_obstruction(&self, world: &World) -> bool {
        super::water_animal::check_spawn_obstruction(self, world)
    }

    fn mob_max_air_supply(&self) -> i32 {
        NAUTILUS_TOTAL_AIR_SUPPLY
    }

    fn base_tick(&self) {
        if self.mob_entity.is_no_ai() {
            self.mob_entity.living_entity.tick_air_supply(self);
        } else {
            self.handle_air_supply(self.get_entity().air_supply.load(Ordering::Relaxed));
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        tick_abstract_nautilus(
            self,
            &self.dash_cooldown,
            self.is_dashing(),
            |dashing| self.set_dashing(dashing),
            || self.get_dash_ready_sound(),
        );
    }

    fn on_damage_with_context(
        &self,
        _damage_type: DamageType,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if let Some(attacker) = cause.or(source)
            && attacker.get_living_entity().is_some()
        {
            self.set_anger_target(attacker);
        }
        true
    }

    fn tick_links(&self) -> Vec<i32> {
        let angry_at = self
            .mob_entity
            .try_with_brain(self, |brain| brain.get(types::ANGRY_AT).copied())
            .flatten();
        let world = self.get_entity().world.load();
        angry_at
            .and_then(|uuid| find_by_uuid(&world, uuid))
            .map(|entity| entity.get_entity().entity_id)
            .into_iter()
            .collect()
    }

    fn can_be_leashed(&self) -> bool {
        !self.is_aggravated()
    }

    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    fn requires_custom_persistence(&self) -> bool {
        self.is_tame()
    }

    fn can_dispenser_equip_into_slot(&self, slot: &EquipmentSlot) -> bool {
        *slot == EquipmentSlot::BODY
            || *slot == EquipmentSlot::SADDLE
            || self.mob_entity.can_pick_up_loot()
    }

    fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        nautilus_can_use_slot(self, slot)
    }

    fn get_equip_sound(&self, slot: &EquipmentSlot) -> Option<Sound> {
        nautilus_equip_sound(self.get_entity(), slot)
    }

    fn mob_controlling_passenger(&self) -> Option<Arc<dyn EntityBase>> {
        self.saddled_controlling_passenger(None)
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::nautilus::DASH,
            self.is_dashing(),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.mob_entity
            .read_legacy_saddle(nbt.get_bool("Saddled") == Some(true));
    }

    fn mob_interact_with_hand(
        &self,
        _player: &Arc<Player>,
        _item_stack: &mut ItemStack,
        _hand: pumpkin_util::Hand,
    ) -> Option<bool> {
        self.mob_entity.set_persistence_required();
        None
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        interact_abstract_nautilus(self, player, item_stack, || {
            self.open_custom_inventory_screen(player);
            true
        })
    }
}

pub(super) fn use_nautilus_player_item(player: &Arc<Player>, item_stack: &mut ItemStack) {
    if item_stack
        .get_item()
        .has_tag(&tag::Item::MINECRAFT_NAUTILUS_BUCKET_FOOD)
    {
        create_filled_result(
            item_stack,
            player,
            ItemStack::new(1, &Item::WATER_BUCKET),
            true,
        );
    } else {
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
    }
}

fn apply_effects(mob: &dyn Mob) {
    let Some(passenger) = mob.get_mob_entity().first_passenger() else {
        return;
    };
    let Some(player) = passenger.get_player() else {
        return;
    };
    let has_effect = player
        .living_entity
        .has_effect(&StatusEffect::BREATH_OF_THE_NAUTILUS);
    let should_refresh = mob.get_entity().world.load().get_world_age() % EFFECT_REFRESH_RATE == 0;
    if !has_effect || should_refresh {
        player.add_effect(Effect {
            effect_type: &StatusEffect::BREATH_OF_THE_NAUTILUS,
            duration: EFFECT_DURATION,
            amplifier: 0,
            ambient: true,
            show_particles: true,
            show_icon: true,
            blend: true,
        });
    }
}

pub(super) fn set_abstract_nautilus_dashing(
    entity: &Entity,
    dashing_flag: &AtomicBool,
    dash_cooldown: &AtomicI32,
    dash_data: TrackedData,
    dashing: bool,
) {
    let changed = dashing_flag.swap(dashing, Ordering::Relaxed) != dashing;
    entity.set_synced_data(dash_data, dashing);
    if changed && entity.tick_count.load(Ordering::Relaxed) > 0 {
        let _ = dash_cooldown.compare_exchange(
            0,
            DASH_COOLDOWN_TICKS,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
}

pub(super) fn handle_abstract_nautilus_start_jump(
    mob: &dyn Mob,
    dash_sound: Sound,
    set_dashing: impl FnOnce(bool),
) {
    mob.make_sound(dash_sound);
    mob.get_entity()
        .emit_own_game_event(mob, GameEvent::EntityAction);
    set_dashing(true);
}

pub(super) fn tick_abstract_nautilus(
    mob: &dyn Mob,
    dash_cooldown: &AtomicI32,
    is_dashing: bool,
    set_dashing: impl FnOnce(bool),
    dash_ready_sound: impl FnOnce() -> Sound,
) {
    apply_effects(mob);
    if is_dashing
        && dash_cooldown.load(Ordering::Relaxed) < DASH_COOLDOWN_TICKS - DASH_MINIMUM_DURATION_TICKS
    {
        set_dashing(false);
    }
    let cooldown = dash_cooldown.load(Ordering::Relaxed);
    if cooldown > 0 {
        dash_cooldown.store(cooldown - 1, Ordering::Relaxed);
        if cooldown == 1 {
            mob.make_sound(dash_ready_sound());
        }
    }
}

fn try_to_tame<M: TamableAnimal>(mob: &M, player: &Arc<Player>) {
    let entity = mob.get_entity();
    let world = entity.world.load();
    if mob.get_random().random_range(0..TAMING_CHANCE) == 0 && mob.approve_taming(player) {
        TamableAnimal::tame(mob, player.gameprofile.id);
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        world.send_entity_status(
            entity,
            EntityStatus::TamingSucceeded,
            ActorEventID::TamingSucceeded,
        );
    } else {
        world.send_entity_status(
            entity,
            EntityStatus::TamingFailed,
            ActorEventID::TamingFailed,
        );
    }
    mob.play_eating_sound();
}

fn do_player_ride(mob: &dyn Mob, player: &Arc<Player>) {
    let entity = mob.get_entity();
    if !entity.has_passengers()
        && let Some(vehicle) = entity.world.load().get_entity_by_id(entity.entity_id)
    {
        entity.add_passenger(vehicle, player.clone());
    }
    if !entity.has_passengers() {
        mob.get_mob_entity().clear_home();
    }
}

pub(super) fn interact_abstract_nautilus<M: TamableAnimal>(
    mob: &M,
    player: &Arc<Player>,
    item_stack: &mut ItemStack,
    open_inventory: impl FnOnce() -> bool,
) -> bool {
    if let Some(result) = mob.handle_important_interactions(player, item_stack) {
        return result;
    }
    if mob.get_mob_entity().living_entity.is_baby() {
        return mob.animal_interact(player, item_stack);
    }
    let is_tame = mob.is_tame();
    let sneaking = player.get_entity().is_sneaking();
    if is_tame && sneaking && open_inventory() {
        return true;
    }
    if !item_stack.is_empty() {
        if !is_tame && mob.is_food(item_stack) {
            mob.use_player_item(player, item_stack);
            try_to_tame(mob, player);
            return true;
        }
        let living = &mob.get_mob_entity().living_entity;
        if mob.is_food(item_stack) && living.health.load() < living.get_max_health() {
            mob.feed(player, item_stack, FEED_HEALING_FACTOR, FEED_DEFAULT_HEAL);
            return true;
        }
        if equip_on_target(mob, player, item_stack) {
            return true;
        }
    }
    if is_tame && !sneaking && !mob.is_food(item_stack) {
        do_player_ride(mob, player);
        return true;
    }
    mob.animal_interact(player, item_stack)
}

pub(super) fn nautilus_can_use_slot(mob: &dyn Mob, slot: &EquipmentSlot) -> bool {
    (*slot != EquipmentSlot::SADDLE && *slot != EquipmentSlot::BODY)
        || (mob.get_mob_entity().living_entity.is_alive()
            && !mob.get_mob_entity().living_entity.is_baby()
            && mob.is_tamed())
}

pub(super) fn nautilus_equip_sound(entity: &Entity, slot: &EquipmentSlot) -> Option<Sound> {
    if *slot != EquipmentSlot::SADDLE {
        return None;
    }
    Some(if entity.is_under_water() {
        Sound::ItemNautilusSaddleUnderwaterEquip
    } else {
        Sound::ItemNautilusSaddleEquip
    })
}
