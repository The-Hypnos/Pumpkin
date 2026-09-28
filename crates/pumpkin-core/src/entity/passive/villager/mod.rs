use crossbeam::atomic::AtomicCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Weak};
use uuid::Uuid;

use crate::block::blocks::bed::BedBlock;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::WhiteBedLikeProperties as BedProperties;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::{EntityPose, EntityStatus, EntityType};
use pumpkin_data::item::{Item, JavaToBedrockItemMapping};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Enchantment as EnchantmentTag, Taggable};
use pumpkin_data::{Block, Enchantment, tracked_data};
use pumpkin_inventory::SimpleInventory;
use pumpkin_inventory::merchant::merchant_screen_handler::MerchantScreenHandler;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::bedrock::{
    client::set_actor_data::{MetadataValue, SyncedActorDataList, entity_data_key},
    server::actor_event::ActorEventID,
};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{CMerchantOffers, Metadata};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3, wrap_degrees};
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::memory::{GlobalPos, MemoryModuleType, PackedMemories, types};
use crate::entity::ai::brain::sensing::golem::golem_detected;
use crate::entity::ai::brain::{Brain, BrainTick, Schedule};
use crate::entity::item::ItemEntity;
use crate::entity::player::Player;
use crate::entity::spawn_util::{SpawnStrategy, try_spawn_mob};
use crate::entity::{
    Entity, EntityBase,
    ai::brain::behavior::utils::{
        DEFAULT_THROW_HAND_Y_DISTANCE, DEFAULT_THROW_VELOCITY, global_pos_in, is_alive, throw_item,
    },
    experience_orb::ExperienceOrbEntity,
    mob::{Mob, MobEntity},
    passive::iron_golem::IronGolemEntity,
};
use crate::world::World;
use crate::world::poi_manager::PoiType;

pub mod ai;
pub mod behaviors;
pub mod data;
pub mod gossip;
pub use data::{
    BREEDING_FOOD_THRESHOLD, GossipType, VillagerData, VillagerProfession, VillagerType,
    get_food_points,
};
use gossip::{GossipContainer, GossipEntry, ReputationEventType};

/// Vanilla `Villager.MAX_GOSSIP_TOPICS`.
const MAX_GOSSIP_TOPICS: usize = 10;
/// Vanilla `Villager.GOSSIP_COOLDOWN`.
const GOSSIP_COOLDOWN: i64 = 1200;
/// Vanilla `Villager.GOSSIP_DECAY_INTERVAL`.
const GOSSIP_DECAY_INTERVAL: i64 = 24000;
/// Vanilla `Villager.HOW_FAR_AWAY_TO_TALK_TO_OTHER_VILLAGERS_ABOUT_GOLEMS`.
const HOW_FAR_AWAY_TO_TALK_TO_OTHER_VILLAGERS_ABOUT_GOLEMS: f64 = 10.0;
/// Vanilla `Villager.HOW_MANY_VILLAGERS_NEED_TO_AGREE_TO_SPAWN_A_GOLEM`.
const HOW_MANY_VILLAGERS_NEED_TO_AGREE_TO_SPAWN_A_GOLEM: usize = 5;
/// Vanilla `Villager.TIME_SINCE_SLEEPING_FOR_GOLEM_SPAWNING`.
const TIME_SINCE_SLEEPING_FOR_GOLEM_SPAWNING: i64 = 24000;
const INVENTORY_SIZE: usize = 8;
/// Vanilla `VillagerMakeLove.breed` ages both parents to this before they can breed again.
const PARENT_BREEDING_COOLDOWN: i32 = 6000;

/// The villager behind `entity`, if it is one.
#[must_use]
pub fn as_villager(entity: &dyn EntityBase) -> Option<&VillagerEntity> {
    entity.cast_any().downcast_ref::<VillagerEntity>()
}

pub(crate) fn trigger_trade_advancement(player: &Player) {
    player.trigger_advancement(
        crate::entity::player::advancement::trigger::AdvancementTrigger::TradedWithVillager,
    );
}

fn enchanted_book_offer_items(
    rng: &mut impl rand::Rng,
) -> Option<(ItemStack, ItemStack, Option<ItemStack>)> {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::StoredEnchantmentsImpl;
    use rand::{RngExt, seq::IndexedRandom};
    use std::borrow::Cow;

    let enchantment_id = *EnchantmentTag::MINECRAFT_TRADEABLE.1.choose(rng)? as u8;
    let enchantment = Enchantment::from_id(enchantment_id)?;
    let level = rng.random_range(1..=enchantment.max_level);
    let mut emeralds = 2 + rng.random_range(0..5 + level * 10) + 3 * level;
    if enchantment.has_tag(&EnchantmentTag::MINECRAFT_DOUBLE_TRADE_PRICE) {
        emeralds *= 2;
    }

    let output = ItemStack::new_with_component(
        1,
        &Item::ENCHANTED_BOOK,
        vec![(
            DataComponent::StoredEnchantments,
            Some(Box::new(StoredEnchantmentsImpl {
                enchantment: Cow::Owned(vec![(enchantment, level)]),
            })),
        )],
    );

    Some((
        ItemStack::new(emeralds.min(64) as u8, &Item::EMERALD),
        output,
        Some(ItemStack::new(1, &Item::BOOK)),
    ))
}

fn enchant_trade_item(
    rng: &mut impl rand::Rng,
    item: &'static Item,
    min_level: i32,
    max_level: i32,
) -> Option<(ItemStack, i32)> {
    use pumpkin_data::data_component_impl::EnchantableImpl;
    use rand::RngExt;

    let mut stack = ItemStack::new(1, item);
    let enchantability = stack
        .get_data_component::<EnchantableImpl>()
        .map_or(1, |value| value.value);
    let additional_cost = rng.random_range(min_level..=max_level);
    let mut level = additional_cost
        + 1
        + rng.random_range(0..=enchantability / 4)
        + rng.random_range(0..=enchantability / 4);
    let bonus = (rng.random::<f32>() + rng.random::<f32>() - 1.0) * 0.15;
    level = ((level as f32 + level as f32 * bonus).round() as i32).max(1);

    let mut available = EnchantmentTag::MINECRAFT_ON_TRADED_EQUIPMENT
        .1
        .iter()
        .filter_map(|id| Enchantment::from_id(*id as u8))
        .filter(|enchantment| enchantment.can_enchant(item))
        .filter_map(|enchantment| {
            (1..=enchantment.max_level)
                .rev()
                .find(|candidate_level| {
                    (enchantment.min_cost.calculate(*candidate_level)
                        ..=enchantment.max_cost.calculate(*candidate_level))
                        .contains(&level)
                })
                .map(|candidate_level| (enchantment, candidate_level))
        })
        .collect::<Vec<_>>();

    if available.is_empty() {
        return None;
    }

    while !available.is_empty() {
        let total_weight: i32 = available
            .iter()
            .map(|(enchantment, _)| enchantment.weight)
            .sum();
        let mut choice = rng.random_range(0..total_weight);
        let chosen_index = available
            .iter()
            .position(|(enchantment, _)| {
                choice -= enchantment.weight;
                choice < 0
            })
            .unwrap_or(0);
        let (enchantment, enchantment_level) = available[chosen_index];
        stack.enchant(enchantment, enchantment_level);

        if rng.random_range(0..50) > level {
            break;
        }
        available.retain(|(candidate, _)| candidate.are_compatible(enchantment));
        level /= 2;
    }
    Some((stack, additional_cost))
}

pub(crate) fn apply_random_dye(rng: &mut impl rand::Rng, stack: &mut ItemStack) {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{DataComponentImpl, DyedColorImpl};
    use rand::RngExt;

    const COLORS: [i32; 16] = [
        0xF9FFFE, 0xF9801D, 0xC74EBD, 0x3AB3DA, 0xFED83D, 0x80C71F, 0xF38BAA, 0x474F52, 0x9D9D97,
        0x169C9C, 0x8932B8, 0x3C44AA, 0x835432, 0x5E7C16, 0xB02E26, 0x1D1D21,
    ];
    let dye_count = 1 + i32::from(rng.random_bool(0.75)) + i32::from(rng.random_bool(0.75));
    let mut channels = [0; 3];
    let mut brightness = 0;
    for _ in 0..dye_count {
        let color = COLORS[rng.random_range(0..COLORS.len())];
        let rgb = [(color >> 16) & 255, (color >> 8) & 255, color & 255];
        brightness += rgb[0].max(rgb[1]).max(rgb[2]);
        for (total, value) in channels.iter_mut().zip(rgb) {
            *total += value;
        }
    }
    let mut rgb = channels.map(|channel| channel / dye_count);
    let average_brightness = brightness / dye_count;
    let max_channel = rgb[0].max(rgb[1]).max(rgb[2]);
    for channel in &mut rgb {
        *channel = average_brightness * *channel / max_channel;
    }
    let color = (rgb[0] << 16) | (rgb[1] << 8) | rgb[2];
    stack.patch.push((
        DataComponent::DyedColor,
        Some(DyedColorImpl { rgb: color }.to_dyn()),
    ));
}

pub(crate) fn apply_random_stew_effect(rng: &mut impl rand::Rng, stack: &mut ItemStack) {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{
        DataComponentImpl, SuspiciousStewEffect, SuspiciousStewEffectsImpl,
    };
    use rand::RngExt;
    use std::borrow::Cow;

    const EFFECTS: [(&str, i32); 6] = [
        ("minecraft:night_vision", 100),
        ("minecraft:jump_boost", 160),
        ("minecraft:weakness", 140),
        ("minecraft:blindness", 120),
        ("minecraft:poison", 280),
        ("minecraft:saturation", 7),
    ];
    let (effect, duration) = EFFECTS[rng.random_range(0..EFFECTS.len())];
    stack.patch.push((
        DataComponent::SuspiciousStewEffects,
        Some(
            SuspiciousStewEffectsImpl {
                effects: Cow::Owned(vec![SuspiciousStewEffect {
                    effect: Cow::Owned(effect.to_owned()),
                    duration,
                }]),
            }
            .to_dyn(),
        ),
    ));
}

pub(crate) fn apply_potion(stack: &mut ItemStack, potion_name: &str) {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{DataComponentImpl, PotionContentsImpl};

    let Some(potion) = pumpkin_data::potion::Potion::from_name(
        potion_name
            .strip_prefix("minecraft:")
            .unwrap_or(potion_name),
    ) else {
        return;
    };
    stack.patch.push((
        DataComponent::PotionContents,
        Some(
            PotionContentsImpl {
                potion_id: Some(i32::from(potion.id)),
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            }
            .to_dyn(),
        ),
    ));
}

/// Resolves a trade destination to its structure set, structure, fallback map
/// name and decoration. 26.3 prefixes these with `#` and renamed several, so
/// both spellings are accepted.
fn explorer_map_target(
    destination: &str,
) -> Option<(
    &'static str,
    pumpkin_data::structures::StructureKeys,
    &'static str,
    i32,
)> {
    use pumpkin_data::structures::StructureKeys;

    let destination = destination.strip_prefix('#').unwrap_or(destination);
    Some(match destination {
        "minecraft:on_jungle_pyramid_maps" | "minecraft:on_jungle_explorer_maps" => (
            "jungle_temples",
            StructureKeys::JunglePyramid,
            "filled_map.explorer_jungle",
            32,
        ),
        "minecraft:on_swamp_hut_maps" | "minecraft:on_swamp_explorer_maps" => (
            "swamp_huts",
            StructureKeys::SwampHut,
            "filled_map.explorer_swamp",
            33,
        ),
        "minecraft:on_desert_village_maps" => (
            "villages",
            StructureKeys::VillageDesert,
            "filled_map.village_desert",
            27,
        ),
        "minecraft:on_plains_village_maps" => (
            "villages",
            StructureKeys::VillagePlains,
            "filled_map.village_plains",
            28,
        ),
        "minecraft:on_savanna_village_maps" => (
            "villages",
            StructureKeys::VillageSavanna,
            "filled_map.village_savanna",
            29,
        ),
        "minecraft:on_snowy_village_maps" => (
            "villages",
            StructureKeys::VillageSnowy,
            "filled_map.village_snowy",
            30,
        ),
        "minecraft:on_taiga_village_maps" => (
            "villages",
            StructureKeys::VillageTaiga,
            "filled_map.village_taiga",
            31,
        ),
        "minecraft:on_ocean_monument_maps" | "minecraft:on_ocean_explorer_maps" => (
            "ocean_monuments",
            StructureKeys::Monument,
            "filled_map.monument",
            9,
        ),
        "minecraft:on_buried_trial_chambers_maps" | "minecraft:on_trial_chambers_maps" => (
            "trial_chambers",
            StructureKeys::TrialChambers,
            "filled_map.trial_chambers",
            34,
        ),
        "minecraft:on_woodland_mansion_maps" | "minecraft:on_woodland_explorer_maps" => (
            "woodland_mansions",
            StructureKeys::Mansion,
            "filled_map.mansion",
            8,
        ),
        _ => return None,
    })
}

pub struct VillagerEntity {
    pub mob_entity: MobEntity,
    pub villager_data: std::sync::Mutex<VillagerData>,
    pub food_level: AtomicI32,
    pub xp: AtomicI32,
    pub last_restock_time: AtomicI64,
    pub last_restock_check_day: AtomicI64,
    pub restocks_today: AtomicI32,
    pub last_gossip_decay_time: AtomicI64,
    pub last_gossip_time: AtomicI64,
    pub gossips: std::sync::Mutex<GossipContainer>,
    pub inventory: std::sync::Mutex<Vec<ItemStack>>,
    pub merchant_inventory: Arc<SimpleInventory>,
    pub offers: std::sync::Mutex<Vec<pumpkin_protocol::java::client::play::MerchantOffer>>,
    pub merchant_update_timer: AtomicI32,
    pub unhappy_counter: AtomicI32,
    pub trade_sound_cooldown: AtomicI32,
    pub increase_profession_level_on_update: AtomicBool,
    pub last_traded_player: std::sync::Mutex<Option<Uuid>>,
    pub trading_player: std::sync::Mutex<Option<(Uuid, u8)>>,
    pub is_trading: AtomicBool,
    /// Vanilla `Entity.tickCount`; the entity age here doubles as the baby timer.
    tick_count: AtomicI32,
    sleeping_pos: AtomicCell<Option<BlockPos>>,
    /// Vanilla rebuilds the brain on the spot; here it waits until the brain tick that asked
    /// for it has let go of the brain.
    brain_refresh_pending: AtomicBool,
    brain_is_baby: AtomicBool,
    death_handled: AtomicBool,
    mirrors: BrainMirrors,
    pub self_weak: std::sync::Mutex<Option<Weak<Self>>>,
}

/// Brain memories other villagers read, copied out after every brain tick so that no villager
/// ever locks another villager's brain.
#[derive(Default)]
struct BrainMirrors {
    job_site: AtomicCell<Option<BlockPos>>,
    potential_job_site: AtomicCell<Option<BlockPos>>,
    interaction_target: AtomicCell<Option<i32>>,
    last_slept: AtomicCell<Option<i64>>,
    golem_detected: AtomicBool,
}

impl VillagerEntity {
    fn bedrock_metadata(data: VillagerData, xp: i32) -> SyncedActorDataList {
        const PROFESSIONS: [i32; 15] = [0, 8, 11, 6, 7, 1, 2, 4, 12, 5, 13, 14, 3, 10, 9];
        const REGIONS: [i32; 7] = [1, 2, 0, 3, 4, 5, 6];

        let mut metadata = SyncedActorDataList::new();
        metadata.set(
            entity_data_key::VARIANT,
            MetadataValue::Int(
                usize::try_from(data.profession.0)
                    .ok()
                    .and_then(|id| PROFESSIONS.get(id))
                    .copied()
                    .unwrap_or_default(),
            ),
        );
        metadata.set(
            entity_data_key::MARK_VARIANT,
            MetadataValue::Int(
                usize::try_from(data.r#type.0)
                    .ok()
                    .and_then(|id| REGIONS.get(id))
                    .copied()
                    .unwrap_or_default(),
            ),
        );
        metadata.set(
            entity_data_key::TRADE_TIER,
            MetadataValue::Int(data.level.0.saturating_sub(1)),
        );
        metadata.set(entity_data_key::MAX_TRADE_TIER, MetadataValue::Int(4));
        metadata.set(entity_data_key::TRADE_EXPERIENCE, MetadataValue::Int(xp));
        metadata
    }

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let villager_data = VillagerData::new(VillagerType::Plains, VillagerProfession::None, 1);

        let villager = Self {
            mob_entity,
            villager_data: std::sync::Mutex::new(villager_data),
            food_level: AtomicI32::new(0),
            xp: AtomicI32::new(0),
            last_restock_time: AtomicI64::new(0),
            last_restock_check_day: AtomicI64::new(0),
            restocks_today: AtomicI32::new(0),
            last_gossip_decay_time: AtomicI64::new(0),
            last_gossip_time: AtomicI64::new(0),
            gossips: std::sync::Mutex::new(GossipContainer::default()),
            inventory: std::sync::Mutex::new(vec![ItemStack::EMPTY.clone(); INVENTORY_SIZE]),
            merchant_inventory: Arc::new(SimpleInventory::new(3)),
            offers: std::sync::Mutex::new(Vec::new()),
            merchant_update_timer: AtomicI32::new(0),
            unhappy_counter: AtomicI32::new(0),
            trade_sound_cooldown: AtomicI32::new(0),
            increase_profession_level_on_update: AtomicBool::new(false),
            last_traded_player: std::sync::Mutex::new(None),
            trading_player: std::sync::Mutex::new(None),
            is_trading: AtomicBool::new(false),
            tick_count: AtomicI32::new(0),
            sleeping_pos: AtomicCell::new(None),
            brain_refresh_pending: AtomicBool::new(false),
            brain_is_baby: AtomicBool::new(false),
            death_handled: AtomicBool::new(false),
            mirrors: BrainMirrors::default(),
            self_weak: std::sync::Mutex::new(None),
        };
        let mob_arc = Arc::new(villager);
        *mob_arc
            .self_weak
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&mob_arc));
        let mut navigator = mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_can_open_doors(true);
        navigator.set_can_float(true);
        navigator.set_required_path_length(48.0);
        drop(navigator);
        mob_arc.mob_entity.set_can_pick_up_loot(true);
        mob_arc.mob_entity.init_brain(mob_arc.as_ref());

        let bedrock_metadata = Self::bedrock_metadata(villager_data, 0);
        mob_arc
            .get_entity()
            .set_synced_data(tracked_data::villager::VILLAGER_DATA, villager_data);
        mob_arc
            .get_entity()
            .send_bedrock_actor_data(&bedrock_metadata);

        mob_arc
    }

    #[must_use]
    pub fn profession(&self) -> VillagerProfession {
        self.villager_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .profession_enum()
    }

    #[must_use]
    pub fn count_food_points_in_inventory(&self) -> i32 {
        self.with_inventory(|inventory| {
            inventory
                .iter()
                .filter(|stack| !stack.is_empty())
                .map(|stack| get_food_points(stack.get_item()) * i32::from(stack.item_count))
                .sum()
        })
    }

    /// Vanilla `Villager.eatUntilFull`.
    pub fn eat_until_full(&self) {
        if self.food_level.load(Ordering::Relaxed) >= BREEDING_FOOD_THRESHOLD {
            return;
        }
        self.with_inventory(|inventory| {
            for stack in inventory.iter_mut() {
                let points = get_food_points(stack.get_item());
                if stack.is_empty() || points == 0 {
                    continue;
                }
                while !stack.is_empty() {
                    self.food_level.fetch_add(points, Ordering::Relaxed);
                    stack.decrement(1);
                    if self.food_level.load(Ordering::Relaxed) >= BREEDING_FOOD_THRESHOLD {
                        return;
                    }
                }
            }
        });
    }

    pub fn set_villager_data(&self, data: VillagerData) {
        let old_profession = {
            let mut villager_data = self
                .villager_data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let old_profession = villager_data.profession;
            *villager_data = data;
            old_profession
        };
        let bedrock_metadata = Self::bedrock_metadata(data, self.xp.load(Ordering::Relaxed));
        self.get_entity()
            .set_synced_data(tracked_data::villager::VILLAGER_DATA, data);
        self.get_entity().send_bedrock_actor_data(&bedrock_metadata);

        if old_profession != data.profession {
            self.offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
    }

    fn create_explorer_map(&self, destination: &str, given: &ItemStack) -> Option<ItemStack> {
        use pumpkin_data::data_component::DataComponent;
        use pumpkin_data::data_component_impl::{DataComponentImpl, ItemNameImpl, MapIdImpl};
        use pumpkin_data::structures::StructureSet;
        use pumpkin_world::generation::generator::structure_finder::find_nearest_structure_start;

        let (structure_set, structure, name, icon_type) = explorer_map_target(destination)?;

        let world = self.get_entity().world.load().clone();
        let generator = world.level.world_gen();
        let target = find_nearest_structure_start(
            self.get_entity().block_pos.load(),
            StructureSet::get(structure_set)?,
            &[structure],
            100,
            &generator,
        )?;
        let server = world.server.upgrade()?;
        let map_id = server.next_map_id();
        let map = server.map_manager.create_map(
            map_id,
            world.dimension.clone(),
            target.0.x,
            target.0.z,
            2,
        );
        map.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .decorations
            .push(crate::world::map::MapDecoration {
                icon_type,
                x: 0,
                z: 0,
                direction: 8,
                display_name: None,
            });

        // 26.3 trades hand out a dedicated map item that already carries its own
        // name; older data trades a plain filled map that has to be named here.
        let mut stack = if given.is_empty() || given.item.id == Item::FILLED_MAP.id {
            let mut stack = ItemStack::new(1, &Item::FILLED_MAP);
            stack.patch.push((
                DataComponent::ItemName,
                Some(ItemNameImpl { name: name.into() }.to_dyn()),
            ));
            stack
        } else {
            given.clone()
        };
        stack.patch.push((
            DataComponent::MapId,
            Some(MapIdImpl { id: map_id }.to_dyn()),
        ));
        Some(stack)
    }

    #[allow(clippy::too_many_lines)]
    pub fn add_trades(&self, profession: VillagerProfession, level: i32) {
        use crate::data::datapack::trade_loader::{DynamicTradeModifier, DynamicVillagerTradeSet};
        use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
        use rand::seq::IndexedRandom;
        use rand::{RngExt, SeedableRng, rngs::StdRng};
        use std::borrow::Cow;

        let villager_type = self
            .villager_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .type_enum();
        let mut new_offers = Vec::new();

        let trade_set = self
            .get_entity()
            .world
            .load()
            .server
            .upgrade()
            .and_then(|server| {
                server
                    .datapack_manager
                    .get_villager_trade_set(profession.to_name(), level)
            })
            .or_else(|| {
                profession
                    .trade_set(level)
                    .map(DynamicVillagerTradeSet::from)
            });

        if let Some(trade_set) = trade_set {
            let mut rng = StdRng::from_rng(&mut rand::rng());
            let mut remaining_trades = trade_set.trades.clone();
            let mut added = 0;
            while added < trade_set.amount && !remaining_trades.is_empty() {
                let index = rng.random_range(0..remaining_trades.len());
                let trade = remaining_trades.remove(index);
                if !trade.allowed_types.is_empty() && !trade.allowed_types.contains(&villager_type)
                {
                    continue;
                }
                let mut base_cost_a = ItemStack::new(trade.wants.count as u8, trade.wants.item);
                let mut output = ItemStack::new(trade.gives.count as u8, trade.gives.item);
                let mut cost_b = trade
                    .wants_b
                    .as_ref()
                    .map(|b| ItemStack::new(b.count as u8, b.item));

                match &trade.modifier {
                    DynamicTradeModifier::None => {}
                    DynamicTradeModifier::EnchantRandomly => {
                        let Some(items) = enchanted_book_offer_items(&mut rng) else {
                            continue;
                        };
                        (base_cost_a, output, cost_b) = items;
                    }
                    DynamicTradeModifier::EnchantWithLevels { min, max } => {
                        let Some((enchanted, additional_cost)) =
                            enchant_trade_item(&mut rng, trade.gives.item, *min, *max)
                        else {
                            continue;
                        };
                        output = enchanted;
                        let count = i32::from(base_cost_a.item_count)
                            .saturating_add(additional_cost)
                            .clamp(0, i32::from(base_cost_a.get_max_stack_size()));
                        if count == 0 {
                            continue;
                        }
                        base_cost_a.set_count(count as u8);
                    }
                    DynamicTradeModifier::ExplorationMap { destination } => {
                        let Some(map) = self.create_explorer_map(destination, &output) else {
                            continue;
                        };
                        output = map;
                    }
                    DynamicTradeModifier::RandomDyes => apply_random_dye(&mut rng, &mut output),
                    DynamicTradeModifier::RandomPotion => {
                        let Some(potion_name) = pumpkin_data::tag::Potion::MINECRAFT_TRADEABLE
                            .0
                            .choose(&mut rng)
                        else {
                            continue;
                        };
                        apply_potion(&mut output, potion_name);
                    }
                    DynamicTradeModifier::SuspiciousStew => {
                        apply_random_stew_effect(&mut rng, &mut output);
                    }
                    DynamicTradeModifier::Potion(potion) => apply_potion(&mut output, potion),
                }
                new_offers.push(pumpkin_protocol::java::client::play::MerchantOffer {
                    base_cost_a: ItemStackSerializer(Cow::Owned(base_cost_a)),
                    output: ItemStackSerializer(Cow::Owned(output)),
                    cost_b: cost_b.map(|stack| ItemStackSerializer(Cow::Owned(stack))),
                    reward_exp: true,
                    uses: 0,
                    max_uses: trade.max_uses,
                    xp: trade.xp,
                    special_price: 0,
                    price_multiplier: trade.price_multiplier,
                    demand: 0,
                });
                added += 1;
            }
        }

        self.offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(new_offers);
    }

    pub fn generate_trades(&self, profession: VillagerProfession, level: i32) {
        self.offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.add_trades(profession, level);
    }

    fn update_special_prices(&self, player: &Player) {
        let player_uuid = player.get_entity().entity_uuid;
        let reputation = self
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_reputation(&player_uuid, |_| true);
        let hero_amplifier = player
            .living_entity
            .get_effect(&StatusEffect::HERO_OF_THE_VILLAGE)
            .map(|effect| i32::from(effect.amplifier));

        let mut offers = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for offer in offers.iter_mut() {
            offer.special_price = -((reputation as f32 * offer.price_multiplier).floor() as i32);
            if let Some(amplifier) = hero_amplifier {
                let discount = ((0.3 + 0.0625 * f64::from(amplifier))
                    * f64::from(offer.base_cost_a.0.item_count))
                .floor() as i32;
                offer.special_price -= discount.max(1);
            }
        }
    }

    fn reset_special_prices(&self) {
        for offer in self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter_mut()
        {
            offer.special_price = 0;
        }
    }

    fn can_continue_trading(
        &self,
        inventory_player: &dyn InventoryPlayer,
        player_uuid: Uuid,
        sync_id: u8,
    ) -> bool {
        let Some(player) = inventory_player.as_any().downcast_ref::<Player>() else {
            return false;
        };
        let entity = self.get_entity();
        let range = player
            .living_entity
            .get_attribute_value(&Attributes::ENTITY_INTERACTION_RANGE)
            + 4.0;
        entity.is_alive()
            && self.mob_entity.living_entity.health.load() > 0.0
            && self
                .trading_player
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|(uuid, id)| *uuid == player_uuid && *id == sync_id)
            && entity
                .bounding_box
                .load()
                .squared_magnitude(player.eye_position())
                < range * range
    }

    fn complete_trade(&self, offer_index: usize, world: &Arc<World>, player_uuid: Uuid) {
        let (xp_gain, reward_exp) = {
            let mut offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(offer) = offers.get_mut(offer_index) else {
                return;
            };
            offer.uses += 1;
            (offer.xp, offer.reward_exp)
        };

        let current_xp = self.xp.fetch_add(xp_gain, Ordering::Relaxed) + xp_gain;
        let villager_data = *self
            .villager_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bedrock_metadata = Self::bedrock_metadata(villager_data, current_xp);
        self.get_entity()
            .set_synced_data(tracked_data::villager::VILLAGER_DATA, villager_data);
        self.get_entity().send_bedrock_actor_data(&bedrock_metadata);

        if reward_exp {
            ExperienceOrbEntity::spawn(world, self.get_entity().pos.load(), xp_gain as u32);
        }

        // Vanilla `rewardTradeXp`: the trade gossip and happy particles follow in the next AI step.
        *self
            .last_traded_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(player_uuid);
        if let Some(player) = world.get_player_by_uuid(player_uuid) {
            self.resend_offers_to_player(&player);
        }
    }

    fn resend_offers_to_player(&self, player: &Arc<Player>) {
        let trading_player = *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((player_uuid, sync_id)) = trading_player else {
            return;
        };
        if player.get_entity().entity_uuid != player_uuid {
            return;
        }
        let offers = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let villager_data = *self
            .villager_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let ok = {
            let screen = player
                .current_screen_handler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let mut screen = screen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if screen.sync_id() != sync_id {
                false
            } else if let Some(handler) =
                screen.as_any_mut().downcast_mut::<MerchantScreenHandler>()
            {
                handler.offers.clone_from(&offers);
                handler.update_result_slot();
                true
            } else {
                false
            }
        };
        if !ok {
            return;
        }
        self.send_trade_offers(player, sync_id, &offers, villager_data);
    }

    fn resend_offers_to_trading_player(&self) {
        let trading_player = *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((player_uuid, sync_id)) = trading_player else {
            return;
        };
        let world = self.get_entity().world.load();
        let Some(player) = world.get_player_by_uuid(player_uuid) else {
            return;
        };
        let offers = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let villager_data = *self
            .villager_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let ok = {
            let screen = player
                .current_screen_handler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let mut screen = screen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if screen.sync_id() != sync_id {
                false
            } else if let Some(handler) =
                screen.as_any_mut().downcast_mut::<MerchantScreenHandler>()
            {
                handler.offers.clone_from(&offers);
                handler.update_result_slot();
                true
            } else {
                false
            }
        };
        if !ok {
            return;
        }
        self.send_trade_offers(&player, sync_id, &offers, villager_data);
    }

    /// Vanilla `Villager.maybeDecayGossip`.
    fn maybe_decay_gossip(&self, game_time: i64) {
        let last_decay = self.last_gossip_decay_time.load(Ordering::Relaxed);
        if last_decay == 0 {
            self.last_gossip_decay_time
                .store(game_time, Ordering::Relaxed);
        } else if game_time >= last_decay + GOSSIP_DECAY_INTERVAL {
            self.gossips
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .decay();
            self.last_gossip_decay_time
                .store(game_time, Ordering::Relaxed);
        }
    }

    fn notify_trading_player_offers_updated(&self) {
        if self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
            && let Some(villager) = self
                .self_weak
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(Weak::upgrade)
        {
            villager.resend_offers_to_trading_player();
        }
    }

    #[must_use]
    pub fn villager_data(&self) -> VillagerData {
        *self
            .villager_data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn villager_xp(&self) -> i32 {
        self.xp.load(Ordering::Relaxed)
    }

    /// Vanilla `setVillagerData(getVillagerData().withProfession(...))`.
    pub fn set_profession(&self, profession: VillagerProfession) {
        let mut data = self.villager_data();
        data.profession = VarInt(profession as i32);
        self.set_villager_data(data);
    }

    /// Vanilla `refreshBrain`, run once the current brain tick has let go of the brain.
    pub fn request_brain_refresh(&self) {
        self.brain_refresh_pending.store(true, Ordering::Relaxed);
    }

    /// Vanilla `refreshBrain`: stops every behaviour and rebuilds the brain from its saved
    /// memories, for a new profession or age.
    fn refresh_brain(&self) {
        let world = self.get_entity().world.load_full();
        let time = world.tick_world_age();
        let mut brain = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        brain.stop_all(&world, self, time);
        let packed = brain.pack();
        *brain = self.make_brain(&packed);
    }

    /// Vanilla `setAge(-24000)` on a newborn.
    fn set_baby(&self) {
        let entity = self.get_entity();
        entity
            .age
            .store(crate::entity::ageable::BABY_START_AGE, Ordering::Relaxed);
        entity.set_synced_data(tracked_data::villager::BABY_ID, true);
        self.refresh_brain();
    }

    #[must_use]
    pub fn mirrored_job_site(&self) -> Option<BlockPos> {
        self.mirrors.job_site.load()
    }

    #[must_use]
    pub fn mirrored_potential_job_site(&self) -> Option<BlockPos> {
        self.mirrors.potential_job_site.load()
    }

    #[must_use]
    pub fn mirrored_interaction_target(&self) -> Option<i32> {
        self.mirrors.interaction_target.load()
    }

    #[must_use]
    pub fn tick_count(&self) -> i32 {
        self.tick_count.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_sleeping(&self) -> bool {
        self.sleeping_pos.load().is_some()
    }

    fn set_sleeping_pos(&self, pos: Option<BlockPos>) {
        self.sleeping_pos.store(pos);
        self.get_entity()
            .set_synced_data(tracked_data::villager::SLEEPING_POS_ID, pos);
    }

    /// Vanilla `LivingEntity.startSleeping`.
    pub fn start_sleeping(&self, tick: &BrainTick<'_>, pos: BlockPos) -> bool {
        let world = tick.world;
        let (block, state) = world.get_block_and_state(&pos);
        if !block.has_tag(&tag::Block::MINECRAFT_BEDS) {
            return false;
        }
        let Some(sleep_height) = BedBlock::sleep_height(state) else {
            return false;
        };
        let entity = self.get_entity();
        entity.set_pos(Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y) + sleep_height + 0.125,
            f64::from(pos.0.z) + 0.5,
        ));
        BedBlock::set_occupied(true, world, block, &pos, state.id);
        entity.set_pose(EntityPose::Sleeping);
        self.set_sleeping_pos(Some(pos));
        entity.velocity.store(Vector3::default());
        true
    }

    /// Vanilla `Villager.stopSleeping`.
    pub fn stop_sleeping(&self, tick: &mut BrainTick<'_>) {
        self.leave_bed(tick.world);
        tick.brain.set(types::LAST_WOKEN, tick.time);
    }

    /// Vanilla `LivingEntity.stopSleeping`: frees the bed and stands up beside it.
    fn leave_bed(&self, world: &Arc<World>) {
        let entity = self.get_entity();
        if let Some(bed) = self.sleeping_pos.load()
            && world.is_loaded(&bed)
        {
            let (block, state) = world.get_block_and_state(&bed);
            if block.has_tag(&tag::Block::MINECRAFT_BEDS) {
                let facing = BedProperties::from_state_id(state.id).facing;
                BedBlock::set_occupied(false, world, block, &bed, state.id);
                let bed_bottom_center = Vector3::new(
                    f64::from(bed.0.x) + 0.5,
                    f64::from(bed.0.y),
                    f64::from(bed.0.z) + 0.5,
                );
                let stand_up = BedBlock::find_stand_up_position(
                    entity.entity_type,
                    world,
                    &bed,
                    facing,
                    entity.yaw.load(),
                )
                .unwrap_or_else(|| {
                    Vector3::new(
                        bed_bottom_center.x,
                        f64::from(bed.0.y + 1) + 0.1,
                        bed_bottom_center.z,
                    )
                });
                let look = (bed_bottom_center - stand_up).normalize();
                let yaw = wrap_degrees(look.z.atan2(look.x).to_degrees() as f32 - 90.0);
                entity.set_pos(stand_up);
                entity.set_rotation(yaw, 0.0);
            }
        }
        entity.set_pose(EntityPose::Standing);
        self.set_sleeping_pos(None);
    }

    /// Vanilla `LivingEntity.checkBedExists`, which wakes a sleeper whose bed is gone.
    fn check_bed_exists(&self) {
        let Some(bed) = self.sleeping_pos.load() else {
            return;
        };
        let world = self.get_entity().world.load_full();
        let state = world.get_block_state(&bed);
        if world.get_block(&bed).has_tag(&tag::Block::MINECRAFT_BEDS)
            && BedBlock::sleep_height(state).is_some()
        {
            return;
        }
        self.leave_bed(&world);
        self.mob_entity.with_brain(self, |tick| {
            tick.brain.set(types::LAST_WOKEN, tick.time);
        });
    }

    /// Vanilla `createPath(pos, range)` followed by `canReach`.
    #[must_use]
    pub fn can_reach(&self, pos: BlockPos, range: i32) -> bool {
        let mob_entity = &self.mob_entity;
        mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .create_path_to_any(&mob_entity.living_entity, &[pos], range)
            .is_some_and(|path| path.can_reach())
    }

    /// Vanilla `golemSpawnConditionsMet` and `wantsToSpawnGolem`.
    const fn wants_to_spawn_golem(
        last_slept: Option<i64>,
        golem_detected: bool,
        game_time: i64,
    ) -> bool {
        match last_slept {
            Some(slept) => {
                game_time - slept < TIME_SINCE_SLEEPING_FOR_GOLEM_SPAWNING && !golem_detected
            }
            None => false,
        }
    }

    /// Vanilla `spawnGolemIfNeeded`: enough recently rested villagers nearby call an iron golem.
    pub fn spawn_golem_if_needed(
        &self,
        tick: &mut BrainTick<'_>,
        villagers_needed_to_agree: usize,
    ) {
        let time = tick.time;
        let own_wants = Self::wants_to_spawn_golem(
            tick.brain.get(types::LAST_SLEPT).copied(),
            tick.brain
                .has_memory_value(types::GOLEM_DETECTED_RECENTLY.id()),
            time,
        );
        if !own_wants {
            return;
        }
        let body_id = self.get_entity().entity_id;
        let search = self.get_entity().bounding_box.load().expand(
            HOW_FAR_AWAY_TO_TALK_TO_OTHER_VILLAGERS_ABOUT_GOLEMS,
            HOW_FAR_AWAY_TO_TALK_TO_OTHER_VILLAGERS_ABOUT_GOLEMS,
            HOW_FAR_AWAY_TO_TALK_TO_OTHER_VILLAGERS_ABOUT_GOLEMS,
        );
        let mut nearby: Vec<Arc<dyn EntityBase>> = Vec::new();
        tick.world
            .entity_grid
            .load()
            .for_each_in_box(&search, |entity| {
                if as_villager(entity.as_ref()).is_some() {
                    nearby.push(Arc::clone(entity));
                }
            });
        let agreeing = nearby
            .iter()
            .filter(|entity| {
                entity.get_entity().entity_id == body_id
                    || as_villager(entity.as_ref()).is_some_and(|other| {
                        Self::wants_to_spawn_golem(
                            other.mirrors.last_slept.load(),
                            other.mirrors.golem_detected.load(Ordering::Relaxed),
                            time,
                        )
                    })
            })
            .take(HOW_MANY_VILLAGERS_NEED_TO_AGREE_TO_SPAWN_A_GOLEM)
            .count();
        if agreeing < villagers_needed_to_agree {
            return;
        }
        let spawned = try_spawn_mob(
            &EntityType::IRON_GOLEM,
            IronGolemEntity::new,
            tick.world,
            &self.get_entity().block_pos.load(),
            10,
            8,
            6,
            SpawnStrategy::LegacyIronGolem,
            false,
        );
        if spawned.is_none() {
            return;
        }
        for villager in nearby {
            if villager.get_entity().entity_id == body_id {
                golem_detected(tick.brain);
            } else if let Some(mob) = villager.as_mob_entity() {
                mob.post_to_brain(Box::new(|other| golem_detected(other.brain)));
            }
        }
    }

    /// Vanilla `Villager.POI_MEMORIES`: which POI types each POI memory may hold.
    fn poi_memory_accepts(&self, memory: MemoryModuleType<GlobalPos>, poi_type: PoiType) -> bool {
        let id = memory.id();
        if id == types::HOME.id() {
            poi_type == PoiType::Home
        } else if id == types::JOB_SITE.id() {
            behaviors::holds_job_site(self.profession(), poi_type)
        } else if id == types::POTENTIAL_JOB_SITE.id() {
            poi_type.is_in(&tag::PointOfInterestType::MINECRAFT_ACQUIRABLE_JOB_SITE)
        } else if id == types::MEETING_POINT.id() {
            poi_type == PoiType::Meeting
        } else {
            false
        }
    }

    /// Vanilla `releasePoi`.
    pub fn release_poi(&self, tick: &BrainTick<'_>, memory: MemoryModuleType<GlobalPos>) {
        let Some(global) = tick.brain.get(memory).copied() else {
            return;
        };
        // Only POIs in this villager's own world are released; vanilla looks the other world up.
        if global_pos_in(tick.world, global.pos) != Some(global) {
            return;
        }
        let poi_manager = &tick.world.poi_manager;
        if poi_manager
            .get_type(&global.pos)
            .is_some_and(|poi_type| self.poi_memory_accepts(memory, poi_type))
        {
            poi_manager.release(tick.world, &global.pos);
        }
    }

    /// Vanilla `releaseAllPois`.
    fn release_all_pois(&self, tick: &BrainTick<'_>) {
        self.release_poi(tick, types::HOME);
        self.release_poi(tick, types::JOB_SITE);
        self.release_poi(tick, types::POTENTIAL_JOB_SITE);
        self.release_poi(tick, types::MEETING_POINT);
    }

    pub fn play_work_sound(&self) {
        if let Some(sound) = self.profession().work_sound() {
            self.get_entity().play_sound(sound);
        }
    }

    /// Vanilla `shouldRestock`.
    pub fn should_restock(&self, tick: &BrainTick<'_>) -> bool {
        let game_time = tick.time;
        let half_day_passed_time = self.last_restock_time.load(Ordering::Relaxed) + 12_000;
        let current_day = tick.world.tick_time_of_day() / 24_000;
        let last_check_day = self
            .last_restock_check_day
            .swap(current_day, Ordering::Relaxed);
        let is_new_day = game_time > half_day_passed_time
            || (last_check_day > 0 && current_day > last_check_day);
        if is_new_day {
            self.last_restock_time.store(game_time, Ordering::Relaxed);
            self.reset_number_of_restocks();
        }
        self.allowed_to_restock(game_time) && self.needs_to_restock()
    }

    /// Vanilla `resetNumberOfRestocks`, with `catchUpDemand` folded in.
    fn reset_number_of_restocks(&self) {
        let missed_updates = 2 - self.restocks_today.load(Ordering::Relaxed);
        {
            let mut offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if missed_updates > 0 {
                for offer in offers.iter_mut() {
                    offer.reset_uses();
                }
            }
            for _ in 0..missed_updates {
                for offer in offers.iter_mut() {
                    offer.update_demand();
                }
            }
        }
        self.notify_trading_player_offers_updated();
        self.restocks_today.store(0, Ordering::Relaxed);
    }

    fn allowed_to_restock(&self, game_time: i64) -> bool {
        let restocks_today = self.restocks_today.load(Ordering::Relaxed);
        restocks_today == 0
            || (restocks_today < 2
                && game_time > self.last_restock_time.load(Ordering::Relaxed) + 2_400)
    }

    fn needs_to_restock(&self) -> bool {
        self.offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(pumpkin_protocol::java::client::play::MerchantOffer::needs_restock)
    }

    /// Vanilla `restock`.
    pub fn restock(&self, game_time: i64) {
        {
            let mut offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for offer in offers.iter_mut() {
                offer.update_demand();
            }
            for offer in offers.iter_mut() {
                offer.reset_uses();
            }
        }
        self.notify_trading_player_offers_updated();
        self.last_restock_time.store(game_time, Ordering::Relaxed);
        self.restocks_today.fetch_add(1, Ordering::Relaxed);
    }

    /// Vanilla `ShowTradesToPlayer.updateDisplayItems`: what the offers `held` pays for give.
    #[must_use]
    pub fn trade_outputs_costing(&self, held: &ItemStack) -> Vec<ItemStack> {
        self.offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|offer| {
                !offer.is_out_of_stock()
                    && (offer.base_cost_a.0.item.id == held.item.id
                        || offer
                            .cost_b
                            .as_ref()
                            .is_some_and(|cost| cost.0.item.id == held.item.id))
            })
            .map(|offer| offer.output.0.as_ref().clone())
            .collect()
    }

    /// `setItemSlot(MAINHAND, ...)`, how villagers show what they are working with.
    pub fn set_held_item(&self, stack: ItemStack) {
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, stack);
    }

    pub fn with_inventory<R>(&self, f: impl FnOnce(&mut Vec<ItemStack>) -> R) -> R {
        f(&mut self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Vanilla `SimpleContainer.countItem`.
    #[must_use]
    pub fn count_item(&self, item: &Item) -> i32 {
        self.with_inventory(|inventory| count_item(inventory, item))
    }

    /// Vanilla `hasFarmSeeds`.
    #[must_use]
    pub fn has_farm_seeds(&self) -> bool {
        self.with_inventory(|inventory| {
            inventory.iter().any(|stack| {
                !stack.is_empty()
                    && stack
                        .item
                        .has_tag(&tag::Item::MINECRAFT_VILLAGER_PLANTABLE_SEEDS)
            })
        })
    }

    /// Vanilla `WorkAtComposter.makeBread`.
    pub fn make_bread(&self) {
        const MAX_BREAD: i32 = 36;
        const MAX_AMOUNT_OF_BREAD_TO_MAKE: i32 = 3;
        const WHEAT_NEEDED_TO_CRAFT_ONE_BREAD: i32 = 3;
        let bread_i_cant_carry = self.with_inventory(|inventory| {
            if count_item(inventory, &Item::BREAD) > MAX_BREAD {
                return None;
            }
            let how_much_bread_to_make = MAX_AMOUNT_OF_BREAD_TO_MAKE
                .min(count_item(inventory, &Item::WHEAT) / WHEAT_NEEDED_TO_CRAFT_ONE_BREAD);
            if how_much_bread_to_make == 0 {
                return None;
            }
            remove_item_type(
                inventory,
                &Item::WHEAT,
                how_much_bread_to_make * WHEAT_NEEDED_TO_CRAFT_ONE_BREAD,
            );
            Some(add_item(
                inventory,
                ItemStack::new(how_much_bread_to_make as u8, &Item::BREAD),
            ))
        });
        if let Some(leftover) = bread_i_cant_carry {
            self.mob_entity
                .living_entity
                .entity
                .spawn_at_location(leftover);
        }
    }

    /// `HarvestFarmland`'s replanting: sows the first plantable seed in the inventory at `pos`.
    pub fn plant_seed(&self, tick: &BrainTick<'_>, pos: BlockPos) {
        let planted = self.with_inventory(|inventory| {
            inventory.iter_mut().find_map(|stack| {
                if stack.is_empty()
                    || !stack
                        .item
                        .has_tag(&tag::Item::MINECRAFT_VILLAGER_PLANTABLE_SEEDS)
                {
                    return None;
                }
                let block = Block::from_item_id(stack.item.id)?;
                stack.decrement(1);
                Some(block)
            })
        });
        let Some(block) = planted else {
            return;
        };
        tick.world
            .set_block_state(&pos, block.default_state.id, BlockFlags::NOTIFY_ALL);
        tick.world.play_sound_raw(
            Sound::ItemCropPlant as u16,
            SoundCategory::Blocks,
            &pos.to_f64(),
            1.0,
            1.0,
        );
    }

    /// Vanilla `BoneMealItem.growCrop` with the first bone meal in the inventory.
    pub fn use_bone_meal_on(&self, tick: &BrainTick<'_>, pos: BlockPos) -> bool {
        if self.count_item(&Item::BONE_MEAL) <= 0 {
            return false;
        }
        let (block, state) = tick.world.get_block_and_state_id(&pos);
        if !tick
            .world
            .block_registry
            .bone_meal(block, tick.world, &pos, state)
        {
            return false;
        }
        self.with_inventory(|inventory| {
            if let Some(stack) = inventory
                .iter_mut()
                .find(|stack| stack.item == &Item::BONE_MEAL)
            {
                stack.decrement(1);
            }
        });
        true
    }

    /// Vanilla `hasExcessFood`.
    #[must_use]
    pub fn has_excess_food(&self) -> bool {
        self.count_food_points_in_inventory() >= 2 * BREEDING_FOOD_THRESHOLD
    }

    /// Vanilla `wantsMoreFood`.
    #[must_use]
    pub fn wants_more_food(&self) -> bool {
        self.count_food_points_in_inventory() < BREEDING_FOOD_THRESHOLD
    }

    /// Vanilla `TradeWithVillager.throwHalfStack`.
    pub fn throw_half_stack(
        &self,
        _tick: &BrainTick<'_>,
        predicate: impl Fn(&ItemStack) -> bool,
        target: Vector3<f64>,
    ) {
        const KEEP_AT_LEAST: u8 = 24;
        let to_throw = self.with_inventory(|inventory| {
            inventory.iter_mut().find_map(|stack| {
                if stack.is_empty() || !predicate(stack) {
                    return None;
                }
                let count = if stack.item_count > stack.get_max_stack_size() / 2 {
                    stack.item_count / 2
                } else if stack.item_count > KEEP_AT_LEAST {
                    stack.item_count - KEEP_AT_LEAST
                } else {
                    return None;
                };
                Some(stack.split(count))
            })
        });
        if let Some(stack) = to_throw.filter(|stack| !stack.is_empty()) {
            throw_item(
                self,
                stack,
                target,
                DEFAULT_THROW_VELOCITY,
                DEFAULT_THROW_HAND_Y_DISTANCE,
            );
        }
    }

    /// Vanilla `Villager.canBreed`; the breeding cooldown stands in for vanilla's positive age.
    #[must_use]
    pub fn can_breed(&self) -> bool {
        self.food_level.load(Ordering::Relaxed) + self.count_food_points_in_inventory()
            >= BREEDING_FOOD_THRESHOLD
            && !self.is_sleeping()
            && !is_baby(self)
            && self.mob_entity.breeding_cooldown.load(Ordering::Relaxed) <= 0
    }

    /// Vanilla `eatAndDigestFood`.
    pub fn eat_and_digest_food(&self) {
        self.eat_until_full();
        self.food_level
            .fetch_sub(BREEDING_FOOD_THRESHOLD, Ordering::Relaxed);
    }

    /// Vanilla `gossip`: swaps rumours with `target` at most once a minute.
    pub fn gossip(&self, tick: &mut BrainTick<'_>, target: &Self) {
        let timestamp = tick.time;
        let ready = |last: i64| timestamp < last || timestamp >= last + GOSSIP_COOLDOWN;
        if !ready(self.last_gossip_time.load(Ordering::Relaxed))
            || !ready(target.last_gossip_time.load(Ordering::Relaxed))
        {
            return;
        }
        // Copied first so two villagers gossiping with each other never hold both locks.
        let heard = target
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let new_gossip = self
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .transfer_from(&heard, &mut tick.mob.get_random(), MAX_GOSSIP_TOPICS);
        self.last_gossip_time.store(timestamp, Ordering::Relaxed);
        target.last_gossip_time.store(timestamp, Ordering::Relaxed);
        self.spawn_golem_if_needed(tick, HOW_MANY_VILLAGERS_NEED_TO_AGREE_TO_SPAWN_A_GOLEM);
        if new_gossip > 0
            && let Some(player) = self.get_trading_player()
        {
            self.update_special_prices(&player);
        }
    }

    /// Vanilla `onReputationEventFrom`.
    pub fn on_reputation_event_from(&self, event: ReputationEventType, source: Uuid) {
        {
            let mut gossips = self
                .gossips
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match event {
                ReputationEventType::ZombieVillagerCured => {
                    gossips.add(source, GossipType::MajorPositive, 20);
                    gossips.add(source, GossipType::MinorPositive, 25);
                }
                ReputationEventType::Trade => gossips.add(source, GossipType::Trading, 2),
                ReputationEventType::VillagerHurt => {
                    gossips.add(source, GossipType::MinorNegative, 25);
                }
                ReputationEventType::VillagerKilled => {
                    gossips.add(source, GossipType::MajorNegative, 25);
                }
            }
        }
        if let Some(player) = self.get_trading_player()
            && player.get_entity().entity_uuid == source
        {
            self.update_special_prices(&player);
        }
    }

    /// Vanilla `tellWitnessesThatIWasMurdered`.
    fn tell_witnesses_that_i_was_murdered(tick: &BrainTick<'_>, murderer: Uuid) {
        let ctx = tick.visibility();
        let Some(witnesses) = ctx.brain.get(types::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            return;
        };
        for witness in witnesses.find_all(&ctx, |entity| as_villager(entity.as_ref()).is_some()) {
            if let Some(villager) = as_villager(witness.as_ref()) {
                villager.on_reputation_event_from(ReputationEventType::VillagerKilled, murderer);
            }
        }
    }

    /// `VillagerMakeLove.breed` and `giveBedToChild`, with vanilla `getBreedOffspring`.
    pub fn breed_with(&self, tick: &mut BrainTick<'_>, partner: &Self, bed: BlockPos) -> bool {
        let world = tick.world;
        let biome_roll: f64 = tick.mob.get_random().random();
        let villager_type = if biome_roll < 0.5 {
            data::villager_type_for_biome(world.get_biome(&self.get_entity().block_pos.load()))
        } else if biome_roll < 0.75 {
            self.villager_data().type_enum()
        } else {
            partner.villager_data().type_enum()
        };
        let child = Self::new(Entity::new(
            Arc::clone(world),
            self.get_entity().pos.load(),
            &EntityType::VILLAGER,
        ));
        child.set_villager_data(VillagerData::new(
            villager_type,
            VillagerProfession::None,
            1,
        ));
        self.mob_entity
            .breeding_cooldown
            .store(PARENT_BREEDING_COOLDOWN, Ordering::Relaxed);
        partner
            .mob_entity
            .breeding_cooldown
            .store(PARENT_BREEDING_COOLDOWN, Ordering::Relaxed);
        child.set_baby();
        if let Some(home) = global_pos_in(world, bed) {
            child.mob_entity.with_brain(child.as_ref(), |child_tick| {
                child_tick.brain.set(types::HOME, home);
            });
        }
        world.spawn_entity(Arc::clone(&child) as Arc<dyn EntityBase>);
        world.send_entity_status(child.get_entity(), EntityStatus::LoveHearts, None);
        true
    }

    /// Vanilla `Villager.wantsToPickUp`.
    fn wants_item(&self, stack: &ItemStack) -> bool {
        (stack.item.has_tag(&tag::Item::MINECRAFT_VILLAGER_PICKS_UP)
            || get_food_points(stack.item) > 0
            || self.profession().requested_items().contains(&stack.item))
            && self.with_inventory(|inventory| can_add_item(inventory, stack))
    }

    /// Vanilla `Mob.aiStep`'s item pickup, into the villager's inventory.
    fn pick_up_nearby_items(&self) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        if !self.mob_entity.can_pick_up_loot() || !is_alive(self) {
            return;
        }
        let world = entity.world.load();
        if !world.level_info.load().game_rules.mob_griefing {
            return;
        }
        let reach = entity.bounding_box.load().expand(1.0, 0.0, 1.0);
        let mut candidates: Vec<Arc<dyn EntityBase>> = Vec::new();
        world
            .entity_grid
            .load()
            .for_each_in_box(&reach, |candidate| {
                if candidate.get_item_entity().is_some() {
                    candidates.push(Arc::clone(candidate));
                }
            });
        for candidate in candidates {
            if let Some(item) = candidate.get_item_entity()
                && item.get_entity().is_alive()
                && item.get_pickup_delay() == 0
            {
                self.pick_up_item(item);
            }
        }
    }

    /// Vanilla `InventoryCarrier.pickUpItem`.
    fn pick_up_item(&self, item: &ItemEntity) {
        let (taken, emptied) = {
            let mut stack = item
                .get_item_stack()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if stack.is_empty() || !self.wants_item(&stack) {
                return;
            }
            let remainder = self.with_inventory(|inventory| add_item(inventory, stack.clone()));
            let taken = stack.item_count - remainder.item_count;
            stack.set_count(remainder.item_count);
            (taken, stack.is_empty())
        };
        if taken == 0 {
            return;
        }
        self.mob_entity
            .living_entity
            .pickup(item.get_entity(), u32::from(taken));
        if emptied {
            item.get_entity().remove();
        } else {
            item.init_data_tracker();
        }
    }

    pub fn set_unhappy(&self) {
        let entity = self.get_entity();
        self.unhappy_counter.store(40, Ordering::Relaxed);
        entity.set_synced_data(tracked_data::villager::UNHAPPY_COUNTER, VarInt(40));
        entity.world.load().send_entity_status(
            entity,
            pumpkin_data::entity::EntityStatus::VillagerAngry,
            Some(ActorEventID::VillagerAngry),
        );
        entity.play_sound(pumpkin_data::sound::Sound::EntityVillagerNo);
    }

    pub fn open_trading_screen(&self, player: &Arc<Player>) {
        // Open the merchant screen and then send the current offers packet
        if let Some(sync_id) = player.open_handled_screen(self, None) {
            let offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let villager_data = *self
                .villager_data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.send_trade_offers(player, sync_id, &offers, villager_data);
        }
    }

    fn bedrock_trade_item(stack: &ItemStack, count: u8) -> NbtCompound {
        let mut item = NbtCompound::new();
        if stack.is_empty() {
            return item;
        }
        let Some(mapping) = JavaToBedrockItemMapping::from_java_item_id(stack.item.id) else {
            return item;
        };
        item.put_byte("Count", count as i8);
        item.put_short("Damage", mapping.bedrock_data as i16);
        item.put_string("Name", mapping.bedrock_item.registry_key.to_owned());
        item
    }

    fn bedrock_trade_data(
        offers: &[pumpkin_protocol::java::client::play::MerchantOffer],
        level: i32,
    ) -> NbtCompound {
        use pumpkin_nbt::tag::NbtTag;

        let tier = level.saturating_sub(1).max(0);
        let mut recipes = Vec::with_capacity(offers.len() + usize::from(level < 5));
        for (index, offer) in offers.iter().enumerate() {
            let base_cost = &offer.base_cost_a.0;
            let demand_bonus = (i32::from(base_cost.item_count).saturating_mul(offer.demand) as f32
                * offer.price_multiplier)
                .floor()
                .max(0.0) as i32;
            let adjusted_count = i32::from(base_cost.item_count)
                .saturating_add(demand_bonus)
                .saturating_add(offer.special_price)
                .clamp(1, i32::from(base_cost.get_max_stack_size()))
                as u8;

            let mut recipe = NbtCompound::new();
            recipe.put_int("netId", index as i32 + 1);
            recipe.put_int(
                "maxUses",
                if offer.is_out_of_stock() {
                    0
                } else {
                    offer.max_uses
                },
            );
            recipe.put_int("traderExp", offer.xp);
            recipe.put_float("priceMultiplierA", offer.price_multiplier);
            recipe.put_float("priceMultiplierB", 0.0);
            recipe.put_compound(
                "sell",
                Self::bedrock_trade_item(&offer.output.0, offer.output.0.item_count),
            );
            recipe.put_int("buyCountA", i32::from(base_cost.item_count));
            recipe.put_int(
                "buyCountB",
                offer
                    .cost_b
                    .as_ref()
                    .map_or(0, |cost| i32::from(cost.0.item_count)),
            );
            recipe.put_int("demand", offer.demand);
            recipe.put_int("tier", (index as i32 / 2).min(tier));
            recipe.put_compound("buyA", Self::bedrock_trade_item(base_cost, adjusted_count));
            recipe.put_compound(
                "buyB",
                offer.cost_b.as_ref().map_or_else(NbtCompound::new, |cost| {
                    Self::bedrock_trade_item(&cost.0, cost.0.item_count)
                }),
            );
            recipe.put_int("uses", offer.uses);
            recipe.put_byte("rewardExp", i8::from(offer.reward_exp));
            recipes.push(NbtTag::Compound(recipe));
        }

        // Bedrock uses this hidden next-tier entry to render the villager XP bar.
        if level < 5 {
            let mut recipe = NbtCompound::new();
            recipe.put_int("maxUses", 0);
            recipe.put_int("traderExp", 0);
            recipe.put_float("priceMultiplierA", 0.0);
            recipe.put_float("priceMultiplierB", 0.0);
            recipe.put_int("buyCountA", 0);
            recipe.put_int("buyCountB", 0);
            recipe.put_int("demand", 0);
            recipe.put_int("tier", 5);
            recipe.put_int("uses", 0);
            recipe.put_byte("rewardExp", 0);
            recipes.push(NbtTag::Compound(recipe));
        }

        let mut data = NbtCompound::new();
        data.put_list("Recipes", recipes);
        data.put_list(
            "TierExpRequirements",
            [0, 10, 70, 150, 250]
                .into_iter()
                .enumerate()
                .map(|(tier, xp)| {
                    let mut requirement = NbtCompound::new();
                    requirement.put_int(&tier.to_string(), xp);
                    NbtTag::Compound(requirement)
                })
                .collect(),
        );
        data
    }

    fn send_trade_offers(
        &self,
        player: &Player,
        sync_id: u8,
        offers: &[pumpkin_protocol::java::client::play::MerchantOffer],
        villager_data: VillagerData,
    ) {
        use pumpkin_protocol::{bedrock::client::CUpdateTrade, codec::var_long::VarLong};

        let java = CMerchantOffers::new(
            VarInt(i32::from(sync_id)),
            offers.to_owned(),
            villager_data.level,
            VarInt(self.xp.load(Ordering::Relaxed)),
            true,
            true,
        );
        let bedrock = CUpdateTrade {
            container_id: sync_id,
            r#type: 15,
            size: VarInt(0),
            trader_tier: VarInt(villager_data.level.0.saturating_sub(1)),
            entity_unique_id: VarLong(i64::from(self.get_entity().entity_id)),
            last_trading_player: VarLong(i64::from(player.entity_id())),
            display_name: ScreenHandlerFactory::get_display_name(self).to_pretty_console(),
            use_new_trade_screen: true,
            using_economy_trade: true,
            data: Self::bedrock_trade_data(offers, villager_data.level.0),
        };
        player.client.try_enqueue_packet_editioned(&java, &bedrock);
    }
}

impl ScreenHandlerFactory for VillagerEntity {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<pumpkin_inventory::player::player_inventory::PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let self_weak = self
            .self_weak
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        let server_player = player.as_any().downcast_ref::<Player>();
        let player_uuid =
            server_player.map_or_else(uuid::Uuid::nil, |p| p.get_entity().entity_uuid);
        if let Some(player) = server_player {
            self.update_special_prices(player);
        }
        let offers = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let world = self.get_entity().world.load().clone();

        let mut handler = MerchantScreenHandler::new(
            sync_id,
            player_inventory,
            self.merchant_inventory.clone(),
            offers,
        );

        self.is_trading.store(true, Ordering::Relaxed);
        *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((player_uuid, sync_id));
        let validity_weak = self_weak.clone();
        handler.validity_check = Some(Box::new(move |inventory_player| {
            validity_weak.upgrade().is_some_and(|villager| {
                villager.can_continue_trading(inventory_player, player_uuid, sync_id)
            })
        }));
        let update_weak = self_weak.clone();
        handler.on_trade_updated = Some(Box::new(move |has_result| {
            let Some(villager) = update_weak.upgrade() else {
                return;
            };
            if villager
                .trade_sound_cooldown
                .compare_exchange(0, 20, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                villager.get_entity().play_sound(if has_result {
                    pumpkin_data::sound::Sound::EntityVillagerYes
                } else {
                    pumpkin_data::sound::Sound::EntityVillagerNo
                });
            }
        }));
        let close_weak = self_weak.clone();
        handler.on_close = Some(Box::new(move || {
            if let Some(villager) = close_weak.upgrade() {
                villager.is_trading.store(false, Ordering::Relaxed);
                *villager
                    .trading_player
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                villager.reset_special_prices();
            }
        }));

        handler.on_trade = Some(Box::new(move |offer_index| {
            if let Some(villager) = self_weak.upgrade() {
                villager.complete_trade(offer_index, &world, player_uuid);
            }
        }));

        Some(Arc::new(std::sync::Mutex::new(handler)) as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        let profession = self
            .villager_data
            .try_lock()
            .map_or(VillagerProfession::None, |data| data.profession_enum());
        TextComponent::translate(profession.translation_key(), [])
    }
}

/// Vanilla `SimpleContainer.countItem`.
fn count_item(inventory: &[ItemStack], item: &Item) -> i32 {
    inventory
        .iter()
        .filter(|stack| stack.item.id == item.id)
        .map(|stack| i32::from(stack.item_count))
        .sum()
}

/// Vanilla `SimpleContainer.removeItemType`.
fn remove_item_type(inventory: &mut [ItemStack], item: &Item, mut count: i32) {
    for stack in inventory.iter_mut() {
        if count <= 0 {
            break;
        }
        if stack.item.id == item.id {
            let removed = count.min(i32::from(stack.item_count));
            stack.decrement(removed as u8);
            count -= removed;
        }
    }
}

/// Vanilla `SimpleContainer.canAddItem`.
fn can_add_item(inventory: &[ItemStack], stack: &ItemStack) -> bool {
    inventory.iter().any(|slot| {
        slot.is_empty()
            || (slot.are_items_and_components_equal(stack)
                && slot.item_count < slot.get_max_stack_size())
    })
}

/// Vanilla `SimpleContainer.addItem`: tops up matching stacks, then fills empty slots.
/// Returns what did not fit.
fn add_item(inventory: &mut [ItemStack], mut stack: ItemStack) -> ItemStack {
    for slot in inventory.iter_mut() {
        if stack.is_empty() {
            return stack;
        }
        if !slot.is_empty() && slot.are_items_and_components_equal(&stack) {
            let moved = (slot.get_max_stack_size() - slot.item_count).min(stack.item_count);
            slot.increment(moved);
            stack.decrement(moved);
        }
    }
    for slot in inventory.iter_mut() {
        if stack.is_empty() {
            break;
        }
        if slot.is_empty() {
            *slot = stack;
            return ItemStack::EMPTY.clone();
        }
    }
    stack
}

impl VillagerEntity {
    /// Vanilla `Villager.tick`'s own work, plus the parts of `LivingEntity.tick` villagers need.
    fn villager_tick(&self) {
        self.tick_count.fetch_add(1, Ordering::Relaxed);
        let entity = self.get_entity();
        let world = entity.world.load();

        let unhappy_counter = self.unhappy_counter.load(Ordering::Relaxed);
        if unhappy_counter > 0 {
            let unhappy_counter = unhappy_counter - 1;
            self.unhappy_counter
                .store(unhappy_counter, Ordering::Relaxed);
            entity.set_synced_data(
                tracked_data::villager::UNHAPPY_COUNTER,
                VarInt(unhappy_counter),
            );
        }
        self.trade_sound_cooldown
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cooldown| {
                (cooldown > 0).then_some(cooldown - 1)
            })
            .ok();

        if !self.is_trading.load(Ordering::Relaxed)
            && self.merchant_update_timer.load(Ordering::Relaxed) > 0
            && self.merchant_update_timer.fetch_sub(1, Ordering::Relaxed) == 1
        {
            if self
                .increase_profession_level_on_update
                .swap(false, Ordering::Relaxed)
            {
                let mut data = self.villager_data();
                data.level.0 += 1;
                self.set_villager_data(data);
                self.add_trades(data.profession_enum(), data.level.0);
            }
            self.mob_entity.living_entity.add_effect(Effect {
                effect_type: &StatusEffect::REGENERATION,
                duration: 200,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            });
        }

        self.maybe_decay_gossip(world.tick_world_age());

        // Vanilla `ageBoundaryReached`: a baby that grew up gets an adult brain.
        let baby = entity.age.load(Ordering::Relaxed) < 0;
        if baby != self.brain_is_baby.load(Ordering::Relaxed) {
            entity.set_synced_data(tracked_data::villager::BABY_ID, baby);
            self.request_brain_refresh();
        }

        self.check_bed_exists();
        self.pick_up_nearby_items();
    }

    /// Vanilla `Villager.die`'s bookkeeping, run on the first AI step after death.
    fn on_died(&self) {
        let murderer = self
            .mob_entity
            .living_entity
            .get_kill_credit()
            .map(|killer| killer.get_entity().entity_uuid);
        self.mob_entity.with_brain(self, |tick| {
            if let Some(murderer) = murderer {
                Self::tell_witnesses_that_i_was_murdered(tick, murderer);
            }
            self.release_all_pois(tick);
        });
    }
}

impl Mob for VillagerEntity {
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        {
            let data = self.villager_data();
            let mut villager_data_nbt = NbtCompound::new();
            villager_data_nbt.put_int("Type", data.r#type.0);
            villager_data_nbt.put_int("Profession", data.profession.0);
            villager_data_nbt.put_int("Level", data.level.0);
            nbt.put_compound("VillagerData", villager_data_nbt);
        };

        // Vanilla `AgeableMob` keeps the baby timer and the breeding cooldown in one `Age`.
        let age = self.get_entity().age.load(Ordering::Relaxed);
        nbt.put_int(
            "Age",
            if age < 0 {
                age
            } else {
                self.mob_entity.breeding_cooldown.load(Ordering::Relaxed)
            },
        );
        nbt.put_int("FoodLevel", self.food_level.load(Ordering::Relaxed));
        nbt.put_int("Xp", self.xp.load(Ordering::Relaxed));
        nbt.put_long(
            "LastRestock",
            self.last_restock_time.load(Ordering::Relaxed),
        );
        nbt.put_int("RestocksToday", self.restocks_today.load(Ordering::Relaxed));
        nbt.put_long(
            "LastGossipDecay",
            self.last_gossip_decay_time.load(Ordering::Relaxed),
        );

        let offers = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !offers.is_empty() {
            let mut recipes = Vec::new();
            for offer in offers.iter() {
                let mut recipe = NbtCompound::new();
                let mut buy = NbtCompound::new();
                let mut sell = NbtCompound::new();

                let item_stack: &ItemStack = offer.base_cost_a.0.as_ref();
                item_stack.write_item_stack(&mut buy);
                recipe.put_compound("buy", buy);

                let item_stack: &ItemStack = offer.output.0.as_ref();
                item_stack.write_item_stack(&mut sell);
                recipe.put_compound("sell", sell);

                if let Some(cost_b) = &offer.cost_b {
                    let mut buy_b = NbtCompound::new();
                    let item_stack: &ItemStack = cost_b.0.as_ref();
                    item_stack.write_item_stack(&mut buy_b);
                    recipe.put_compound("buyB", buy_b);
                }

                recipe.put_int("uses", offer.uses);
                recipe.put_int("maxUses", offer.max_uses);
                recipe.put_bool("rewardExp", offer.reward_exp);
                recipe.put_int("xp", offer.xp);
                recipe.put_float("priceMultiplier", offer.price_multiplier);
                recipe.put_int("specialPrice", offer.special_price);
                recipe.put_int("demand", offer.demand);

                recipes.push(NbtTag::Compound(recipe));
            }
            let mut offers_compound = NbtCompound::new();
            offers_compound.put("Recipes", NbtTag::List(recipes));
            nbt.put_compound("Offers", offers_compound);
        }
        drop(offers);

        let inventory_list: Vec<NbtTag> = self.with_inventory(|inventory| {
            inventory
                .iter()
                .filter(|item| !item.is_empty())
                .map(|item| {
                    let mut item_compound = NbtCompound::new();
                    item.write_item_stack(&mut item_compound);
                    NbtTag::Compound(item_compound)
                })
                .collect()
        });
        if !inventory_list.is_empty() {
            nbt.put("Inventory", NbtTag::List(inventory_list));
        }

        let gossip_list: Vec<NbtTag> = self
            .gossips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries()
            .map(|entry| {
                let mut gossip_nbt = NbtCompound::new();
                let (u1, u2) = entry.target.as_u64_pair();
                let uuid_array = vec![(u1 >> 32) as i32, u1 as i32, (u2 >> 32) as i32, u2 as i32];
                gossip_nbt.put("Target", NbtTag::IntArray(uuid_array));
                gossip_nbt.put_string("Type", entry.gossip_type.name().to_string());
                gossip_nbt.put_int("Value", entry.value);
                NbtTag::Compound(gossip_nbt)
            })
            .collect();
        if !gossip_list.is_empty() {
            nbt.put("Gossips", NbtTag::List(gossip_list));
        }
    }

    #[allow(clippy::too_many_lines)]
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(villager_data_nbt) = nbt.get_compound("VillagerData") {
            let mut data = self
                .villager_data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(t) = villager_data_nbt.get_int("Type") {
                data.r#type = VarInt(t);
            }
            if let Some(p) = villager_data_nbt.get_int("Profession") {
                data.profession = VarInt(p);
            }
            if let Some(l) = villager_data_nbt.get_int("Level") {
                data.level = VarInt(l);
            }
        }

        if let Some(age) = nbt.get_int("Age") {
            if age < 0 {
                self.get_entity().age.store(age, Ordering::Relaxed);
            } else {
                self.mob_entity
                    .breeding_cooldown
                    .store(age, Ordering::Relaxed);
            }
        }
        if let Some(food) = nbt.get_int("FoodLevel") {
            self.food_level.store(food, Ordering::Relaxed);
        }
        if let Some(xp) = nbt.get_int("Xp") {
            self.xp.store(xp, Ordering::Relaxed);
        }
        if let Some(restock) = nbt.get_long("LastRestock") {
            self.last_restock_time.store(restock, Ordering::Relaxed);
        }
        if let Some(today) = nbt.get_int("RestocksToday") {
            self.restocks_today.store(today, Ordering::Relaxed);
        }
        if let Some(last_decay) = nbt.get_long("LastGossipDecay") {
            self.last_gossip_decay_time
                .store(last_decay, Ordering::Relaxed);
        }

        if let Some(offers_compound) = nbt.get_compound("Offers")
            && let Some(recipes) = offers_compound.get_list("Recipes")
        {
            let mut offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            offers.clear();
            for tag in recipes {
                if let Some(recipe) = tag.extract_compound() {
                    let buy = recipe
                        .get_compound("buy")
                        .and_then(ItemStack::read_item_stack);
                    let buy_b = recipe
                        .get_compound("buyB")
                        .and_then(ItemStack::read_item_stack);
                    let sell_item = recipe
                        .get_compound("sell")
                        .and_then(ItemStack::read_item_stack);

                    if let (Some(buy), Some(sell_item)) = (buy, sell_item)
                        && !buy.is_empty()
                        && !sell_item.is_empty()
                        && buy_b.as_ref().is_none_or(|stack| !stack.is_empty())
                    {
                        let uses = recipe.get_int("uses").unwrap_or(0);
                        let max_uses = recipe.get_int("maxUses").unwrap_or(12);
                        let reward_exp = recipe.get_bool("rewardExp").unwrap_or(true);
                        let xp = recipe.get_int("xp").unwrap_or(2);
                        let price_multiplier = recipe.get_float("priceMultiplier").unwrap_or(0.05);
                        let special_price = recipe.get_int("specialPrice").unwrap_or(0);
                        let demand = recipe.get_int("demand").unwrap_or(0);

                        offers.push(pumpkin_protocol::java::client::play::MerchantOffer {
                            base_cost_a: buy.into(),
                            output: sell_item.into(),
                            cost_b: buy_b.map(Into::into),
                            reward_exp,
                            uses,
                            max_uses,
                            xp,
                            special_price,
                            price_multiplier,
                            demand,
                        });
                    }
                }
            }
        }

        if let Some(inventory_list) = nbt.get_list("Inventory") {
            self.with_inventory(|inventory| {
                inventory.fill(ItemStack::EMPTY.clone());
                let stacks = inventory_list
                    .iter()
                    .filter_map(|tag| tag.extract_compound())
                    .filter_map(ItemStack::read_item_stack);
                for (slot, stack) in inventory.iter_mut().zip(stacks) {
                    *slot = stack;
                }
            });
        }

        if let Some(gossip_list) = nbt.get_list("Gossips") {
            let mut gossips = self
                .gossips
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            gossips.clear();
            for tag in gossip_list {
                let Some(gossip_nbt) = tag.extract_compound() else {
                    continue;
                };
                let target = gossip_nbt
                    .get_int_array("Target")
                    .filter(|uuid_array| uuid_array.len() == 4)
                    .map(|uuid_array| {
                        Uuid::from_u128(
                            u128::from(uuid_array[0] as u32) << 96
                                | u128::from(uuid_array[1] as u32) << 64
                                | u128::from(uuid_array[2] as u32) << 32
                                | u128::from(uuid_array[3] as u32),
                        )
                    });
                let gossip_type = gossip_nbt
                    .get_string("Type")
                    .and_then(GossipType::from_name)
                    .or_else(|| {
                        gossip_nbt
                            .get_int("Type")
                            .and_then(GossipType::from_legacy_id)
                    });
                if let (Some(target), Some(gossip_type), Some(value)) =
                    (target, gossip_type, gossip_nbt.get_int("Value"))
                {
                    gossips.put(GossipEntry {
                        target,
                        gossip_type,
                        value,
                    });
                }
            }
        }

        // Vanilla `readAdditionalSaveData` ends with `refreshBrain`: the brain was built before
        // the profession and age above were known.
        self.refresh_brain();
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    // Villagers keep their baby timer in the entity age rather than through `AgeableMob`.
    fn mob_is_baby(&self) -> bool {
        self.get_entity().age.load(Ordering::Relaxed) < 0
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        let mut brain = ai::VILLAGER_PROVIDER.make_brain(self, packed);
        // Vanilla `registerBrainGoals`.
        let baby = is_baby(self);
        self.brain_is_baby.store(baby, Ordering::Relaxed);
        brain.set_schedule(if baby {
            Schedule::BabyVillager
        } else {
            Schedule::Villager
        });
        let world = self.get_entity().world.load_full();
        brain.update_activity_from_schedule(&world, world.tick_world_age());
        brain
    }

    fn after_brain_tick(&self, tick: &mut BrainTick<'_>) {
        let mirrors = &self.mirrors;
        mirrors
            .job_site
            .store(tick.brain.get(types::JOB_SITE).map(|site| site.pos));
        mirrors.potential_job_site.store(
            tick.brain
                .get(types::POTENTIAL_JOB_SITE)
                .map(|site| site.pos),
        );
        mirrors.interaction_target.store(
            tick.brain
                .get(types::INTERACTION_TARGET)
                .map(|target| target.get_entity().entity_id),
        );
        mirrors
            .last_slept
            .store(tick.brain.get(types::LAST_SLEPT).copied());
        mirrors.golem_detected.store(
            tick.brain
                .has_memory_value(types::GOLEM_DETECTED_RECENTLY.id()),
            Ordering::Relaxed,
        );
    }

    /// Vanilla `Villager.customServerAiStep`.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if !is_alive(self) {
            self.mob_entity.apply_brain_inbox(self);
            if !self.death_handled.swap(true, Ordering::Relaxed) {
                self.on_died();
            }
            return;
        }
        self.mob_entity.tick_brain(self);
        if self.brain_refresh_pending.swap(false, Ordering::Relaxed) {
            self.refresh_brain();
        }
        let last_traded_player = self
            .last_traded_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let world = self.get_entity().world.load();
        if let Some(player_uuid) = last_traded_player {
            self.on_reputation_event_from(ReputationEventType::Trade, player_uuid);
            world.send_entity_status(
                self.get_entity(),
                EntityStatus::VillagerHappy,
                Some(ActorEventID::VillagerHappy),
            );
        }
        if self.get_random().random_range(0..100) == 0
            && behaviors::raid_at(&world, &self.get_entity().block_pos.load())
                .is_some_and(|raid| raid.active && !raid.over)
        {
            world.send_entity_status(self.get_entity(), EntityStatus::VillagerSweat, None);
        }
    }

    fn wants_to_pick_up(&self, _brain: &Brain, stack: &ItemStack) -> bool {
        self.wants_item(stack)
    }

    fn mob_bedrock_identifier(&self) -> Option<&'static str> {
        Some("minecraft:villager_v2")
    }

    fn mob_java_spawn_metadata(&self, version: JavaMinecraftVersion) -> Option<Box<[u8]>> {
        if version < JavaMinecraftVersion::V_1_9 {
            return None;
        }
        let mut metadata = Vec::new();
        Metadata::new(tracked_data::villager::VILLAGER_DATA, self.villager_data())
            .write(&mut metadata, &version)
            .ok()?;
        metadata.push(255);
        Some(metadata.into_boxed_slice())
    }

    fn mob_bedrock_spawn_metadata(&self) -> Option<SyncedActorDataList> {
        Some(Self::bedrock_metadata(
            self.villager_data(),
            self.xp.load(Ordering::Relaxed),
        ))
    }

    fn clear_trading_player(&self) {
        *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn get_trading_player(&self) -> Option<Arc<Player>> {
        let trading_player = *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (player_uuid, _) = trading_player?;
        self.get_entity()
            .world
            .load()
            .get_player_by_uuid(player_uuid)
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let data = self.villager_data();
        let bedrock_metadata = Self::bedrock_metadata(data, self.xp.load(Ordering::Relaxed));
        entity.set_synced_data(tracked_data::villager::VILLAGER_DATA, data);
        entity.send_bedrock_actor_data(&bedrock_metadata);
        if entity.age.load(Ordering::Relaxed) < 0 {
            entity.set_synced_data(tracked_data::villager::BABY_ID, true);
        }
    }

    /// Vanilla `LivingEntity.hurtServer`'s wake-up, then `Villager.setLastHurtByMob`.
    fn on_damage(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        source: Option<&dyn EntityBase>,
    ) {
        if self.is_sleeping() {
            // The attacker may be ticking on another thread, so the brain hears of it next tick.
            self.leave_bed(&self.get_entity().world.load_full());
            self.mob_entity.post_to_brain(Box::new(|tick| {
                tick.brain.set(types::LAST_WOKEN, tick.time);
            }));
        }
        let Some(source) = source.filter(|source| source.get_living_entity().is_some()) else {
            return;
        };
        self.on_reputation_event_from(
            ReputationEventType::VillagerHurt,
            source.get_entity().entity_uuid,
        );
        if is_alive(self) && source.get_entity().entity_type == &EntityType::PLAYER {
            self.get_entity().world.load().send_entity_status(
                self.get_entity(),
                EntityStatus::VillagerAngry,
                Some(ActorEventID::VillagerAngry),
            );
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.villager_tick();
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // Vanilla leaves a villager spawn egg to the egg, which makes a baby.
        if item_stack.item.id == Item::VILLAGER_SPAWN_EGG.id
            || self.is_trading.load(Ordering::Relaxed)
            || self.is_sleeping()
        {
            return false;
        }
        if self.get_entity().age.load(Ordering::Relaxed) < 0 {
            self.set_unhappy();
            return true;
        }

        let trade_params = {
            let offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if offers.is_empty() {
                let data = self.villager_data();
                (data.profession_enum() != VillagerProfession::None
                    && data.profession_enum() != VillagerProfession::Nitwit)
                    .then(|| (data.profession_enum(), data.level.0))
            } else {
                None
            }
        };
        if let Some((prof, level)) = trade_params {
            self.generate_trades(prof, level);
        }

        let has_offers = !self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if !has_offers {
            self.set_unhappy();
            return true;
        }

        player.increment_stat(
            pumpkin_data::statistic::StatisticCategory::Custom,
            pumpkin_data::statistic::CustomStatistic::TalkedToVillager as i32,
            1,
        );

        let villager = self
            .self_weak
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(villager) = villager {
            villager.open_trading_screen(player);
        }

        true
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use pumpkin_data::data_component_impl::{EnchantmentsImpl, StoredEnchantmentsImpl};
    use pumpkin_data::villager::VillagerTradeModifier;
    use pumpkin_util::version::JavaMinecraftVersion;

    use super::*;

    #[test]
    fn villager_data_metadata_uses_the_villager_tracker_slot() {
        let data = VillagerData::new(VillagerType::Plains, VillagerProfession::Librarian, 1);
        let metadata = Metadata::new(tracked_data::villager::VILLAGER_DATA, data);
        let mut bytes = Vec::new();

        metadata
            .write(&mut bytes, &JavaMinecraftVersion::V_26_3)
            .unwrap();

        assert_eq!(bytes, [19, 18, 2, 9, 1]);
    }

    #[test]
    fn villager_data_maps_to_bedrock_appearance_metadata() {
        let metadata = VillagerEntity::bedrock_metadata(
            VillagerData::new(VillagerType::Plains, VillagerProfession::Librarian, 3),
            75,
        );

        assert!(matches!(
            metadata.0.get(&entity_data_key::VARIANT),
            Some(MetadataValue::Int(5))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::MARK_VARIANT),
            Some(MetadataValue::Int(0))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::TRADE_TIER),
            Some(MetadataValue::Int(2))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::MAX_TRADE_TIER),
            Some(MetadataValue::Int(4))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::TRADE_EXPERIENCE),
            Some(MetadataValue::Int(75))
        ));
    }

    #[test]
    fn unhappy_counter_metadata_uses_the_abstract_villager_tracker_slot() {
        let metadata = Metadata::new(tracked_data::villager::UNHAPPY_COUNTER, VarInt(40));
        let mut bytes = Vec::new();

        metadata
            .write(&mut bytes, &JavaMinecraftVersion::V_26_3)
            .unwrap();

        assert_eq!(bytes, [18, 1, 40]);
    }

    #[test]
    fn enchanted_book_offer_has_vanilla_items_and_a_nonzero_price() {
        let (emeralds, enchanted_book, book) = enchanted_book_offer_items(&mut rand::rng())
            .expect("the generated tradeable-enchantment tag is populated");
        let stored = enchanted_book
            .get_data_component::<StoredEnchantmentsImpl>()
            .unwrap();

        assert_eq!(emeralds.item.id, Item::EMERALD.id);
        assert!((5..=64).contains(&emeralds.item_count));
        assert_eq!(book.unwrap().item.id, Item::BOOK.id);
        assert_eq!(enchanted_book.item.id, Item::ENCHANTED_BOOK.id);
        assert_eq!(stored.enchantment.len(), 1);
        assert!(
            stored.enchantment[0]
                .0
                .has_tag(&EnchantmentTag::MINECRAFT_TRADEABLE)
        );
        assert!((1..=stored.enchantment[0].0.max_level).contains(&stored.enchantment[0].1));
    }

    #[test]
    fn every_exploration_map_destination_resolves() {
        // An unresolved destination makes the trade get skipped silently, so the
        // generated data and the lookup have to stay in step.
        let mut checked = 0;
        for id in 0..=14 {
            let Some(profession) = VillagerProfession::from_i32(id) else {
                continue;
            };
            for level in 1..=5 {
                let Some(trade_set) = profession.trade_set(level) else {
                    continue;
                };
                for trade in trade_set.trades {
                    if let VillagerTradeModifier::ExplorationMap { destination } = trade.modifier {
                        assert!(
                            explorer_map_target(destination).is_some(),
                            "unresolved exploration map destination: {destination}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 0, "no exploration map trades found");
    }

    #[test]
    fn generated_trades_keep_dynamic_modifiers_and_secondary_costs() {
        let librarian = VillagerProfession::Librarian.trade_set(1).unwrap();
        let enchanted_book = librarian
            .trades
            .iter()
            .find(|trade| trade.modifier == VillagerTradeModifier::EnchantRandomly)
            .unwrap();
        assert_eq!(enchanted_book.wants.item, &Item::EMERALD);
        assert_eq!(enchanted_book.wants_b.unwrap().item, &Item::BOOK);
        assert_eq!(enchanted_book.price_multiplier, 0.2);

        let cartographer = VillagerProfession::Cartographer.trade_set(2).unwrap();
        assert!(cartographer.trades.iter().any(|trade| {
            matches!(trade.modifier, VillagerTradeModifier::ExplorationMap { .. })
                && !trade.allowed_types.is_empty()
                && trade
                    .wants_b
                    .is_some_and(|cost| cost.item == &Item::COMPASS)
        }));

        let fletcher = VillagerProfession::Fletcher.trade_set(5).unwrap();
        assert!(fletcher.trades.iter().any(|trade| {
            trade.modifier == VillagerTradeModifier::RandomPotion
                && trade.wants_b.is_some_and(|cost| cost.item == &Item::ARROW)
        }));
    }

    #[test]
    fn smith_trade_sets_include_the_shared_vanilla_trades() {
        let armorer_novice = VillagerProfession::Armorer.trade_set(1).unwrap();
        assert!(armorer_novice.trades.iter().any(|trade| {
            trade.wants.item == &Item::COAL && trade.gives.item == &Item::EMERALD
        }));

        let armorer_apprentice = VillagerProfession::Armorer.trade_set(2).unwrap();
        assert!(armorer_apprentice.trades.iter().any(|trade| {
            trade.wants.item == &Item::EMERALD && trade.gives.item == &Item::BELL
        }));
        assert!(armorer_apprentice.trades.iter().any(|trade| {
            trade.wants.item == &Item::IRON_INGOT && trade.gives.item == &Item::EMERALD
        }));

        for profession in [
            VillagerProfession::Toolsmith,
            VillagerProfession::Weaponsmith,
        ] {
            assert!(profession.trade_set(1).unwrap().trades.iter().any(|trade| {
                trade.wants.item == &Item::COAL && trade.gives.item == &Item::EMERALD
            }));
        }
    }

    #[test]
    fn traded_equipment_is_enchanted_and_reports_its_additional_price() {
        for _ in 0..32 {
            let (stack, additional_price) =
                enchant_trade_item(&mut rand::rng(), &Item::DIAMOND_SWORD, 5, 19).unwrap();
            let enchantments = stack.get_data_component::<EnchantmentsImpl>().unwrap();

            assert!((5..=19).contains(&additional_price));
            assert!(!enchantments.enchantment.is_empty());
        }
    }
}
