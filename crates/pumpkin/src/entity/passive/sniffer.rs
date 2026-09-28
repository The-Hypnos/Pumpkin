use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::brain::behavior::random_look_around::direction_from_rotation;
use crate::entity::ai::brain::behavior::utils::{global_pos_in, is_alive};
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::passive::animal::finalize_spawn_child_from_breeding;
use crate::entity::passive::sniffer_ai;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    item::ItemEntity,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub const SNIFFER_FOOD: &[&Item] = &[&Item::TORCHFLOWER_SEEDS, &Item::PITCHER_POD];
pub const SNIFFER_BABY_START_AGE: i32 = -48000;
pub const DIGGING_DROP_SEED_OFFSET_TICKS: i32 = 120;
const DIGGING_BB_HEIGHT_OFFSET: f32 = 0.4;
const DIGGING_EYE_HEIGHT: f32 = 0.81;
const HEAD_OFFSET: f64 = 2.25;
const MAX_EXPLORED_POSITIONS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum SnifferState {
    #[default]
    Idling = 0,
    FeelingHappy = 1,
    Scenting = 2,
    Sniffing = 3,
    Searching = 4,
    Digging = 5,
    Rising = 6,
}

impl SnifferState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::FeelingHappy,
            2 => Self::Scenting,
            3 => Self::Sniffing,
            4 => Self::Searching,
            5 => Self::Digging,
            6 => Self::Rising,
            _ => Self::Idling,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Represents a Sniffer, a large passive mob that digs up ancient seeds.
///
/// Wiki: <https://minecraft.wiki/w/Sniffer>
pub struct SnifferEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub state: AtomicI32,
    pub drop_seed_at_tick: AtomicI32,
    tick_count: AtomicI32,
}

impl SnifferEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let sniffer = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(SnifferState::Idling.id()),
            drop_seed_at_tick: AtomicI32::new(0),
            tick_count: AtomicI32::new(0),
        });
        sniffer.mob_entity.init_brain(sniffer.as_ref());
        sniffer
    }

    #[must_use]
    pub fn get_state(&self) -> SnifferState {
        SnifferState::from_id(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, state: SnifferState) {
        let previous = SnifferState::from_id(self.state.swap(state.id(), Ordering::Relaxed));
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(state.id()),
        );
        // Vanilla `getDefaultDimensions`: a digging sniffer is shorter. Babies never dig.
        if (previous == SnifferState::Digging) != (state == SnifferState::Digging) {
            let sniffer = &EntityType::SNIFFER;
            let dimensions = if state == SnifferState::Digging {
                EntityDimensions {
                    width: sniffer.dimension[0],
                    height: sniffer.dimension[1] - DIGGING_BB_HEIGHT_OFFSET,
                    eye_height: DIGGING_EYE_HEIGHT,
                }
            } else {
                EntityDimensions {
                    width: sniffer.dimension[0],
                    height: sniffer.dimension[1],
                    eye_height: sniffer.eye_height,
                }
            };
            entity.entity_dimension.store(dimensions);
        }
    }

    fn play_sound(&self, sound: Sound, pitch: f32) {
        let entity = self.get_entity();
        entity.world.load().play_sound_fine(
            sound,
            SoundCategory::Neutral,
            &entity.pos.load(),
            1.0,
            pitch,
        );
    }

    /// Vanilla `Sniffer.transitionTo`.
    pub fn transition_to(&self, state: SnifferState) {
        match state {
            SnifferState::Idling | SnifferState::Searching => self.set_state(state),
            SnifferState::FeelingHappy => {
                self.play_sound(Sound::EntitySnifferHappy, 1.0);
                self.set_state(state);
            }
            SnifferState::Scenting => {
                self.set_state(state);
                self.play_sound(
                    Sound::EntitySnifferScenting,
                    if self.is_baby() { 1.3 } else { 1.0 },
                );
            }
            SnifferState::Sniffing => {
                self.play_sound(Sound::EntitySnifferSniffing, 1.0);
                self.set_state(state);
            }
            SnifferState::Digging => {
                self.set_state(state);
                self.on_digging_start();
            }
            SnifferState::Rising => {
                self.play_sound(Sound::EntitySnifferDiggingStop, 1.0);
                self.set_state(state);
            }
        }
    }

    fn on_digging_start(&self) {
        let entity = self.get_entity();
        let drop_tick = self.tick_count.load(Ordering::Relaxed) + DIGGING_DROP_SEED_OFFSET_TICKS;
        self.drop_seed_at_tick.store(drop_tick, Ordering::Relaxed);
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(drop_tick),
        );
        entity
            .world
            .load()
            .send_entity_status(entity, EntityStatus::SnifferDiggingSound, None);
    }

    /// Vanilla `Sniffer.onDiggingComplete`: remembers the block it stands on.
    pub fn on_digging_complete(&self, tick: &mut BrainTick<'_>, success: bool) {
        if !success {
            return;
        }
        let pos = self.get_entity().pos.load();
        let on_pos = BlockPos::floored(pos.x, pos.y - 0.2, pos.z);
        let Some(explored) = global_pos_in(tick.world, on_pos) else {
            return;
        };
        let mut updated: Vec<_> = tick
            .brain
            .get(types::SNIFFER_EXPLORED_POSITIONS)
            .map(|positions| {
                positions
                    .iter()
                    .take(MAX_EXPLORED_POSITIONS)
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        updated.insert(0, explored);
        tick.brain.set(types::SNIFFER_EXPLORED_POSITIONS, updated);
    }

    /// Vanilla `Sniffer.getHeadPosition`.
    #[must_use]
    pub fn get_head_position(&self) -> Vector3<f64> {
        let entity = self.get_entity();
        let forward = direction_from_rotation(entity.pitch.load(), entity.yaw.load());
        entity.pos.load() + forward * HEAD_OFFSET
    }

    /// Vanilla `Sniffer.getHeadBlock`.
    #[must_use]
    pub fn get_head_block(&self) -> BlockPos {
        let head_pos = self.get_head_position();
        let pos = self.get_entity().pos.load();
        BlockPos::floored(head_pos.x, pos.y + f64::from(0.2f32), head_pos.z)
    }

    /// Vanilla `Sniffer.canDig(BlockPos)`.
    pub fn can_dig_at(&self, tick: &BrainTick<'_>, pos: BlockPos) -> bool {
        if !tick
            .world
            .get_block(&pos)
            .has_tag(&tag::Block::MINECRAFT_SNIFFER_DIGGABLE_BLOCK)
        {
            return false;
        }
        let explored = global_pos_in(tick.world, pos).is_some_and(|global| {
            tick.brain
                .get(types::SNIFFER_EXPLORED_POSITIONS)
                .is_some_and(|positions| positions.contains(&global))
        });
        !explored
            && self
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .can_reach_within(&self.mob_entity.living_entity, pos.to_centered_f64(), 0.0)
    }

    /// Vanilla `Sniffer.dropSeed`.
    fn drop_seed(&self) {
        if self.drop_seed_at_tick.load(Ordering::Relaxed) != self.tick_count.load(Ordering::Relaxed)
        {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        let head = self.get_head_block();
        let head_pos = Vector3::new(
            f64::from(head.0.x),
            f64::from(head.0.y),
            f64::from(head.0.z),
        );
        // TODO: roll the `gameplay/sniffer_digging` gift loot table once loot tables are extracted.
        let seed_item = if rand::random::<bool>() {
            &Item::TORCHFLOWER_SEEDS
        } else {
            &Item::PITCHER_POD
        };
        let item_entity = ItemEntity::new(
            Entity::new(world.clone(), head_pos, &EntityType::ITEM),
            ItemStack::new(1, seed_item),
        );
        world.spawn_entity(Arc::new(item_entity));
        self.play_sound(Sound::EntitySnifferDropSeed, 1.0);
    }
}

impl AgeableMob for SnifferEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn get_baby_start_age(&self) -> i32 {
        SNIFFER_BABY_START_AGE
    }
}

impl Animal for SnifferEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_SNIFFER_FOOD)
            || SNIFFER_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }

    /// Vanilla `Sniffer.canMate`: both sniffers must be idle, scenting or happy.
    fn can_mate(&self, partner: &dyn EntityBase) -> bool {
        let mating_state = |state| {
            matches!(
                state,
                SnifferState::Idling | SnifferState::Scenting | SnifferState::FeelingHappy
            )
        };
        mating_state(self.get_state())
            && partner
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(|partner| mating_state(partner.get_state()))
    }

    /// Vanilla `Sniffer.spawnChildFromBreeding`: lays an egg instead of spawning a baby.
    fn spawn_child_from_breeding(&self, tick: &mut BrainTick<'_>, mate: &dyn EntityBase) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let egg = ItemEntity::new(
            Entity::new(Arc::clone(tick.world), pos, &EntityType::ITEM),
            ItemStack::new(1, &Item::SNIFFER_EGG),
        );
        finalize_spawn_child_from_breeding(self, mate);
        let mut rng = self.get_random();
        let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, 0.5);
        self.play_sound(Sound::BlockSnifferEggPlop, pitch);
        tick.world.spawn_entity(Arc::new(egg));
    }
}

impl Mob for SnifferEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        sniffer_ai::SNIFFER_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        sniffer_ai::update_activity(tick);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_count.fetch_add(1, Ordering::Relaxed);
        self.ageable_ai_step();
        if self.get_state() == SnifferState::Digging {
            self.drop_seed();
        }
    }

    /// Vanilla `Sniffer.die`.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        if self.mob_entity.living_entity.dead.load(Ordering::Relaxed) {
            self.transition_to(SnifferState::Idling);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::sniffer::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(self.get_state().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(self.drop_seed_at_tick.load(Ordering::Relaxed)),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntitySnifferEat)
    }
}
