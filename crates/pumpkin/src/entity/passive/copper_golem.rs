use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, AtomicI64, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_inventory::Inventory;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use rand::RngExt;

use crate::entity::ai::brain::behavior::utils::{
    DEFAULT_THROW_HAND_Y_DISTANCE, DEFAULT_THROW_VELOCITY, is_alive, throw_item,
};
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::{Brain, BrainTick};
use crate::entity::passive::copper_golem_ai;
use crate::entity::{
    Entity, EntityBase,
    custom_sound::CustomSound,
    mob::{Mob, MobEntity},
    player::Player,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum WeatherState {
    #[default]
    Unaffected = 0,
    Exposed = 1,
    Weathered = 2,
    Oxidized = 3,
}

impl WeatherState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Exposed,
            2 => Self::Weathered,
            3 => Self::Oxidized,
            _ => Self::Unaffected,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Unaffected => Self::Exposed,
            Self::Exposed => Self::Weathered,
            Self::Weathered | Self::Oxidized => Self::Oxidized,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Oxidized => Self::Weathered,
            Self::Weathered => Self::Exposed,
            Self::Exposed | Self::Unaffected => Self::Unaffected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum CopperGolemState {
    #[default]
    Idle = 0,
    GettingItem = 1,
    GettingNoItem = 2,
    DroppingItem = 3,
    DroppingNoItem = 4,
}

impl CopperGolemState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::GettingItem,
            2 => Self::GettingNoItem,
            3 => Self::DroppingItem,
            4 => Self::DroppingNoItem,
            _ => Self::Idle,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Represents a Copper Golem, a passive creation mob made of copper blocks and lightning rod.
///
/// Wiki: <https://minecraft.wiki/w/Copper_Golem>
pub struct CopperGolemEntity {
    pub mob_entity: MobEntity,
    pub weather_state: AtomicI32,
    pub state: AtomicI32,
    pub next_weathering_tick: AtomicI64,
    /// The chest this golem holds open, vanilla's `openedChestPos` plus the opener count it adds.
    opened_chest: Mutex<Option<Arc<dyn Inventory>>>,
}

impl CopperGolemEntity {
    const REQUIRED_PATH_LENGTH: f32 = 48.0;

    pub fn new(entity: Entity) -> Arc<Self> {
        let golem = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            weather_state: AtomicI32::new(WeatherState::Unaffected.id()),
            state: AtomicI32::new(CopperGolemState::Idle.id()),
            next_weathering_tick: AtomicI64::new(-1),
            opened_chest: Mutex::new(None),
        });
        let mut navigator = golem
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_required_path_length(Self::REQUIRED_PATH_LENGTH);
        navigator.set_can_open_doors(true);
        drop(navigator);
        golem.mob_entity.init_brain(golem.as_ref());
        golem.mob_entity.with_brain(golem.as_ref(), |tick| {
            let cooldown = tick.mob.get_random().random_range(60..100);
            tick.brain
                .set(types::TRANSPORT_ITEMS_COOLDOWN_TICKS, cooldown);
        });
        golem
    }

    /// Vanilla `container.startOpen(golem)` plus `setOpenedChestPos`.
    pub fn open_chest(&self, container: &Arc<dyn Inventory>) {
        let mut opened = self
            .opened_chest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = opened.take() {
            previous.on_close();
        }
        container.on_open();
        *opened = Some(Arc::clone(container));
    }

    /// Vanilla `container.stopOpen(golem)` plus `clearOpenedChestPos`. Vanilla's opener recheck
    /// closes a chest the golem walked away from; here it happens directly.
    pub fn close_chest(&self) {
        if let Some(container) = self
            .opened_chest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            container.on_close();
        }
    }

    pub fn play_sound(&self, sound: Sound) {
        let entity = self.get_entity();
        entity.world.load().play_sound_fine(
            sound,
            SoundCategory::Neutral,
            &entity.pos.load(),
            1.0,
            1.0,
        );
    }

    fn main_hand_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    #[must_use]
    pub fn get_weather_state(&self) -> WeatherState {
        WeatherState::from_id(self.weather_state.load(Ordering::Relaxed))
    }

    pub fn set_weather_state(&self, state: WeatherState) {
        self.weather_state.store(state.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::copper_golem::WEATHER_STATE,
            VarInt(state.id()),
        );
    }

    #[must_use]
    pub fn get_state(&self) -> CopperGolemState {
        CopperGolemState::from_id(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, state: CopperGolemState) {
        self.state.store(state.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::copper_golem::COPPER_GOLEM_STATE,
            VarInt(state.id()),
        );
    }

    #[must_use]
    pub fn step_sound(&self) -> Sound {
        match self.get_weather_state() {
            WeatherState::Unaffected | WeatherState::Exposed => Sound::EntityCopperGolemStep,
            WeatherState::Weathered => Sound::EntityCopperGolemWeatheredStep,
            WeatherState::Oxidized => Sound::EntityCopperGolemOxidizedStep,
        }
    }
}

impl CustomSound for CopperGolemEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(match self.get_weather_state() {
            WeatherState::Unaffected | WeatherState::Exposed => Sound::EntityCopperGolemHurt,
            WeatherState::Weathered => Sound::EntityCopperGolemWeatheredHurt,
            WeatherState::Oxidized => Sound::EntityCopperGolemOxidizedHurt,
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(match self.get_weather_state() {
            WeatherState::Unaffected | WeatherState::Exposed => Sound::EntityCopperGolemDeath,
            WeatherState::Weathered => Sound::EntityCopperGolemWeatheredDeath,
            WeatherState::Oxidized => Sound::EntityCopperGolemOxidizedDeath,
        })
    }
}

impl Mob for CopperGolemEntity {
    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_long(
            "next_weather_age",
            self.next_weathering_tick.load(Ordering::Relaxed),
        );
        nbt.put_int("weather_state", self.get_weather_state().id());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(next) = nbt.get_long("next_weather_age") {
            self.next_weathering_tick.store(next, Ordering::Relaxed);
        }
        if let Some(state) = nbt.get_int("weather_state") {
            self.set_weather_state(WeatherState::from_id(state));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        copper_golem_ai::COPPER_GOLEM_PROVIDER.make_brain(self, packed)
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        copper_golem_ai::update_activity(tick);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            return;
        }
        self.mob_entity.tick_brain(self);
    }

    /// Vanilla `CopperGolem.actuallyHurt`.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        self.set_state(CopperGolemState::Idle);
        if self.mob_entity.living_entity.dead.load(Ordering::Relaxed) {
            self.close_chest();
        }
    }

    fn mob_on_lightning_strike(
        &self,
        caller: &dyn EntityBase,
        lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
        self.set_weather_state(WeatherState::Unaffected);
        self.mob_entity
            .living_entity
            .on_lightning_strike(caller, lightning);
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::copper_golem::WEATHER_STATE,
            VarInt(self.get_weather_state().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::copper_golem::COPPER_GOLEM_STATE,
            VarInt(self.get_state().id()),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();

        if item_stack.is_empty() {
            let held = self.main_hand_item();
            if !held.is_empty() {
                throw_item(
                    self,
                    held,
                    player.get_entity().pos.load(),
                    DEFAULT_THROW_VELOCITY,
                    DEFAULT_THROW_HAND_Y_DISTANCE,
                );
                self.mob_entity
                    .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
                return true;
            }
        }

        // Honeycomb waxing
        if item_stack.item.id == Item::HONEYCOMB.id
            && self.next_weathering_tick.load(Ordering::Relaxed) != -2
        {
            self.next_weathering_tick.store(-2, Ordering::Relaxed);
            let pos = entity.pos.load();
            world.play_sound(Sound::ItemHoneycombWaxOn, SoundCategory::Blocks, &pos);
            return true;
        }

        // Axe scraping
        if item_stack.item.has_tag(&tag::Item::MINECRAFT_AXES) {
            let current_next = self.next_weathering_tick.load(Ordering::Relaxed);
            if current_next == -2 {
                self.next_weathering_tick.store(-1, Ordering::Relaxed);
                let pos = entity.pos.load();
                world.play_sound(Sound::ItemAxeScrape, SoundCategory::Blocks, &pos);
                return true;
            }

            let weather_state = self.get_weather_state();
            if weather_state != WeatherState::Unaffected {
                self.set_weather_state(weather_state.previous());
                self.next_weathering_tick.store(-1, Ordering::Relaxed);
                let pos = entity.pos.load();
                world.play_sound(Sound::ItemAxeScrape, SoundCategory::Blocks, &pos);
                return true;
            }
        }

        false
    }
}
