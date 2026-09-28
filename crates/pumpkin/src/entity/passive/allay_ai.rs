use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::GameMode;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::seq::IndexedRandom;

use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::utils::throw_item;
use crate::entity::ai::brain::behavior::{self, GoAndGiveItemsToTarget, Timed};
use crate::entity::ai::brain::memory::position_tracker::{
    BlockPosTracker, EntityTracker, PositionTracker,
};
use crate::entity::ai::brain::memory::{MemoryModuleId, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::passive::allay::AllayEntity;

const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET: f32 = 2.25;
const SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM: f32 = 1.75;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.5;
const CLOSE_ENOUGH_TO_TARGET: i32 = 4;
const TOO_FAR_FROM_TARGET: i32 = 16;
const MAX_LOOK_DISTANCE: f32 = 6.0;
const MIN_WAIT_DURATION: i32 = 30;
const MAX_WAIT_DURATION: i32 = 60;
const DISTANCE_TO_WANTED_ITEM: i32 = 32;
const GIVE_ITEM_TIMEOUT_DURATION: i32 = 20;
const THROW_VELOCITY: Vector3<f64> = Vector3::new(0.2, 0.3, 0.2);
const ITEM_PICKUP_COOLDOWN_DURATION: i32 = 60;
const MAX_NOTEBLOCK_DISTANCE: i32 = 1024;
const LIKED_PLAYER_RANGE: f64 = 64.0;
const THROW_SOUND_PITCHES: [f32; 16] = [
    0.5625, 0.625, 0.75, 0.9375, 1.0, 1.0, 1.125, 1.25, 1.5, 1.875, 2.0, 2.25, 2.5, 3.0, 3.75, 4.0,
];

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::new(0.8))),
            Box::new(Timed::new(behavior::AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::LIKED_NOTEBLOCK_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::ITEM_PICKUP_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        0,
        vec![
            Box::new(behavior::go_to_wanted_item(
                |_| true,
                SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM,
                true,
                DISTANCE_TO_WANTED_ITEM,
            )),
            Box::new(Timed::new(GoAndGiveItemsToTarget::new(
                get_item_deposit_position,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET,
                GIVE_ITEM_TIMEOUT_DURATION,
                throw_item_to_target,
                types::ITEM_PICKUP_COOLDOWN_TICKS,
                ITEM_PICKUP_COOLDOWN_DURATION,
                has_item_to_throw,
            ))),
            Box::new(behavior::stay_close_to_target(
                get_item_deposit_position,
                |tick| {
                    !tick
                        .brain
                        .has_memory_value(types::NEAREST_VISIBLE_WANTED_ITEM.id())
                },
                CLOSE_ENOUGH_TO_TARGET,
                TOO_FAR_FROM_TARGET,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET,
            )),
            Box::new(behavior::set_entity_look_target_sometimes(
                |_| true,
                MAX_LOOK_DISTANCE,
                MIN_WAIT_DURATION,
                MAX_WAIT_DURATION,
            )),
            Box::new(behavior::run_one(vec![
                (Box::new(behavior::fly(SPEED_MULTIPLIER_WHEN_IDLING)), 2),
                (
                    Box::new(behavior::set_walk_target_from_look_target(
                        |_| true,
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        3,
                    )),
                    2,
                ),
                (
                    Box::new(behavior::DoNothing::new(
                        MIN_WAIT_DURATION,
                        MAX_WAIT_DURATION,
                    )),
                    1,
                ),
            ])),
        ],
    )
}

fn as_allay<'a>(tick: &BrainTick<'a>) -> Option<&'a AllayEntity> {
    tick.mob.cast_any().downcast_ref::<AllayEntity>()
}

fn has_item_to_throw(tick: &BrainTick<'_>) -> bool {
    as_allay(tick).is_some_and(|allay| !allay.inventory_item().is_empty())
}

/// Vanilla `AllayAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    // Vanilla drops a stale liked note block inside `getItemDepositPosition`; the getter here
    // only reads the brain, so the erase happens once per tick instead.
    if let Some(noteblock) = tick.brain.get(types::LIKED_NOTEBLOCK_POSITION).copied()
        && !should_deposit_items_at_liked_noteblock(tick, &noteblock)
    {
        tick.brain.erase(types::LIKED_NOTEBLOCK_POSITION.id());
    }
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Idle]);
}

/// Vanilla `AllayAi.getItemDepositPosition`.
fn get_item_deposit_position(tick: &BrainTick<'_>) -> Option<Arc<dyn PositionTracker>> {
    if let Some(noteblock) = tick.brain.get(types::LIKED_NOTEBLOCK_POSITION)
        && should_deposit_items_at_liked_noteblock(tick, noteblock)
    {
        return Some(Arc::new(BlockPosTracker::new(noteblock.pos.up())));
    }
    get_liked_player(tick)
        .map(|player| Arc::new(EntityTracker::new(player, true)) as Arc<dyn PositionTracker>)
}

/// Vanilla `AllayAi.shouldDepositItemsAtLikedNoteblock`.
fn should_deposit_items_at_liked_noteblock(
    tick: &BrainTick<'_>,
    noteblock: &crate::entity::ai::brain::memory::GlobalPos,
) -> bool {
    noteblock.is_close_enough(
        tick.world.dimension.minecraft_name,
        &tick.mob.get_entity().block_pos.load(),
        MAX_NOTEBLOCK_DISTANCE,
    ) && tick.world.get_block(&noteblock.pos) == &Block::NOTE_BLOCK
        && tick
            .brain
            .has_memory_value(types::LIKED_NOTEBLOCK_COOLDOWN_TICKS.id())
}

/// Vanilla `AllayAi.getLikedPlayer`.
#[must_use]
pub fn get_liked_player(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let liked = tick.brain.get(types::LIKED_PLAYER)?;
    let player = tick.world.get_player_by_uuid(*liked)?;
    let body_pos = tick.mob.get_entity().pos.load();
    let in_range = player
        .get_entity()
        .pos
        .load()
        .squared_distance_to_vec(&body_pos)
        < LIKED_PLAYER_RANGE * LIKED_PLAYER_RANGE;
    (matches!(
        player.gamemode.load(),
        GameMode::Survival | GameMode::Creative
    ) && in_range)
        .then_some(player as Arc<dyn EntityBase>)
}

/// Vanilla `AllayAi.throwItem`.
fn throw_item_to_target(tick: &mut BrainTick<'_>, target: Vector3<f64>) {
    let Some(allay) = as_allay(tick) else {
        return;
    };
    let item = allay.remove_inventory_item(1);
    if item.is_empty() {
        return;
    }
    throw_item(
        tick.mob,
        item,
        target + Vector3::new(0.0, 1.0, 0.0),
        THROW_VELOCITY,
        0.2,
    );
    let mut rng = tick.mob.get_random();
    if tick.time % 7 == 0 && rng.random::<f64>() < 0.9 {
        let pitch = THROW_SOUND_PITCHES.choose(&mut rng).copied().unwrap_or(1.0);
        tick.world.play_sound_fine(
            Sound::EntityAllayItemThrown,
            SoundCategory::Neutral,
            &tick.mob.get_entity().pos.load(),
            1.0,
            pitch,
        );
    }
}

const MEMORY_TYPES: &[MemoryModuleId] = &[
    types::LIKED_PLAYER.id(),
    types::LIKED_NOTEBLOCK_POSITION.id(),
    types::LIKED_NOTEBLOCK_COOLDOWN_TICKS.id(),
];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestPlayers,
    SensorType::HurtBy,
    SensorType::NearestItems,
];

pub static ALLAY_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| vec![init_core_activity(), init_idle_activity()],
};
