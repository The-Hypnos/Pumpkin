use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, tag};

use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::transport_items_between_containers::{
    ContainerInteractionState, TransportItemTarget, TransportItemsBetweenContainers, is_chest_open,
};
use crate::entity::ai::brain::behavior::{self, Timed};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::passive::copper_golem::{CopperGolemEntity, CopperGolemState};

const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 1.5;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS: i32 = 32;
const TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS: i32 = 8;
const TICK_TO_START_ON_REACHED_INTERACTION: i32 = 1;
const TICK_TO_PLAY_ON_REACHED_SOUND: i32 = 9;
const TICK_TO_STOP_ON_REACHED_INTERACTION: i32 = 60;

fn as_golem<'a>(tick: &BrainTick<'a>) -> Option<&'a CopperGolemEntity> {
    tick.mob.cast_any().downcast_ref::<CopperGolemEntity>()
}

fn is_transport_source(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_COPPER_CHESTS)
}

fn is_transport_destination(block: &Block) -> bool {
    block == &Block::CHEST || block == &Block::TRAPPED_CHEST
}

/// Vanilla `CopperGolemAi.onReachedTargetInteraction`.
fn on_reached_target(
    tick: &mut BrainTick<'_>,
    interaction: ContainerInteractionState,
    target: &TransportItemTarget,
    ticks_since_reaching_target: i32,
) {
    let Some(golem) = as_golem(tick) else {
        return;
    };
    let (state, sound) = match interaction {
        ContainerInteractionState::PickupItem => (
            CopperGolemState::GettingItem,
            Sound::EntityCopperGolemNoItemGet,
        ),
        ContainerInteractionState::PickupNoItem => (
            CopperGolemState::GettingNoItem,
            Sound::EntityCopperGolemNoItemNoGet,
        ),
        ContainerInteractionState::PlaceItem => (
            CopperGolemState::DroppingItem,
            Sound::EntityCopperGolemItemDrop,
        ),
        ContainerInteractionState::PlaceNoItem => (
            CopperGolemState::DroppingNoItem,
            Sound::EntityCopperGolemItemNoDrop,
        ),
    };
    if ticks_since_reaching_target == TICK_TO_START_ON_REACHED_INTERACTION {
        golem.open_chest(&target.container);
        golem.set_state(state);
    }
    if ticks_since_reaching_target == TICK_TO_PLAY_ON_REACHED_SOUND {
        golem.play_sound(sound);
    }
    if ticks_since_reaching_target == TICK_TO_STOP_ON_REACHED_INTERACTION {
        golem.close_chest();
    }
}

/// Vanilla `CopperGolemAi.onTravelling`.
fn on_travelling(tick: &BrainTick<'_>) {
    if let Some(golem) = as_golem(tick) {
        golem.close_chest();
        golem.set_state(CopperGolemState::Idle);
    }
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
            Box::new(behavior::interact_with_door()),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::GAZE_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TRANSPORT_ITEMS_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_pairs(
        Activity::Idle,
        vec![
            (
                0,
                Box::new(Timed::new(TransportItemsBetweenContainers::new(
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    is_transport_source,
                    is_transport_destination,
                    TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS,
                    TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS,
                    on_reached_target,
                    on_travelling,
                    is_chest_open,
                ))),
            ),
            (
                1,
                Box::new(behavior::set_entity_look_target_sometimes(
                    |entity| entity.get_entity().entity_type == &EntityType::PLAYER,
                    6.0,
                    40,
                    80,
                )),
            ),
            (
                2,
                Box::new(behavior::run_one_with_conditions(
                    vec![
                        (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
                        (
                            types::TRANSPORT_ITEMS_COOLDOWN_TICKS.id(),
                            MemoryStatus::ValuePresent,
                        ),
                    ],
                    vec![
                        (
                            Box::new(behavior::stroll_with_range(
                                SPEED_MULTIPLIER_WHEN_IDLING,
                                2,
                                2,
                                true,
                            )),
                            1,
                        ),
                        (Box::new(behavior::DoNothing::new(30, 60)), 1),
                    ],
                )),
            ),
        ],
    )
}

/// Vanilla `CopperGolemAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Idle]);
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[SensorType::NearestLivingEntities, SensorType::HurtBy];

pub static COPPER_GOLEM_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| vec![init_core_activity(), init_idle_activity()],
};
