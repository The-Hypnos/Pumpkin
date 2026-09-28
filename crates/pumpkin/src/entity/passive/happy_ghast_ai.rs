use std::sync::Arc;

use pumpkin_data::environment_attribute::Activity;

use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::{self, BehaviorControl, Timed};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};

const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.1;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
const BABY_GHAST_CLOSE_ENOUGH_DIST: f64 = 3.0;
const ADULT_FOLLOW_RANGE: (i32, i32) = (3, 16);
const SWIM_CHANCE: f32 = 0.8;
const PANIC_FLY_HEIGHT: i32 = 0;

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::new(SWIM_CHANCE))),
            Box::new(Timed::new(behavior::AnimalPanic::flying(
                SPEED_MULTIPLIER_WHEN_PANICKING,
                PANIC_FLY_HEIGHT,
            ))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_pairs(
        Activity::Idle,
        vec![
            (
                1,
                Box::new(Timed::new(behavior::FollowTemptation::with_distance(
                    |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                    |_| BABY_GHAST_CLOSE_ENOUGH_DIST,
                    true,
                ))),
            ),
            (
                2,
                Box::new(behavior::baby_follow(
                    types::NEAREST_VISIBLE_PLAYER.id(),
                    |brain| {
                        brain
                            .get(types::NEAREST_VISIBLE_PLAYER)
                            .map(|player| Arc::clone(player) as Arc<dyn EntityBase>)
                    },
                    ADULT_FOLLOW_RANGE.0,
                    ADULT_FOLLOW_RANGE.1,
                    |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                    true,
                )),
            ),
            (
                3,
                Box::new(behavior::baby_follow(
                    types::NEAREST_VISIBLE_ADULT.id(),
                    |brain| brain.get(types::NEAREST_VISIBLE_ADULT).map(Arc::clone),
                    ADULT_FOLLOW_RANGE.0,
                    ADULT_FOLLOW_RANGE.1,
                    |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                    true,
                )),
            ),
            (
                4,
                Box::new(behavior::run_one(vec![
                    (
                        Box::new(behavior::fly(SPEED_MULTIPLIER_WHEN_IDLING))
                            as Box<dyn BehaviorControl>,
                        1,
                    ),
                    (
                        Box::new(behavior::set_walk_target_from_look_target(
                            |_| true,
                            SPEED_MULTIPLIER_WHEN_IDLING,
                            3,
                        )),
                        1,
                    ),
                ])),
            ),
        ],
    )
}

fn init_panic_activity() -> ActivityData {
    ActivityData::new(
        Activity::Panic,
        Vec::new(),
        vec![(types::IS_PANICKING.id(), MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `HappyGhastAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Panic, Activity::Idle]);
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::HurtBy,
    SensorType::FoodTemptations,
    SensorType::NearestAdultAnyType,
    SensorType::NearestPlayers,
];

pub static HAPPY_GHAST_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_panic_activity(),
        ]
    },
};
