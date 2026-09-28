use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;

use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::one_shot::{OneShot, sequence};
use crate::entity::ai::brain::behavior::{self, Behavior, BehaviorControl, Timed};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::mob::Mob;
use crate::entity::passive::camel::CamelEntity;

const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 4.0;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 2.0;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 2.5;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 2.5;
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 1.0;
const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
const SWIM_CHANCE: f32 = 0.8;
const MINIMAL_SITTING_POSE_SECONDS: i32 = 20;
const LOOK_AROUND_INTERVAL: (i32, i32) = (150, 250);

fn as_camel(mob: &dyn Mob) -> Option<&CamelEntity> {
    mob.cast_any().downcast_ref::<CamelEntity>()
}

fn refuse_to_move(tick: &BrainTick<'_>) -> bool {
    as_camel(tick.mob).is_some_and(|camel| camel.refuse_to_move(tick.time))
}

/// Vanilla `BehaviorBuilder.triggerIf(Predicate.not(Camel::refuseToMove), behavior)`.
fn unless_refusing_to_move(behavior: OneShot) -> Box<dyn BehaviorControl> {
    Box::new(sequence(
        "TriggerIf(!refuseToMove)",
        |tick| !refuse_to_move(tick),
        behavior,
    ))
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::new(SWIM_CHANCE))),
            Box::new(Timed::new(CamelPanic(behavior::AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            )))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::GAZE_COOLDOWN_TICKS,
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
                Box::new(behavior::set_entity_look_target_sometimes(
                    |entity| entity.get_entity().entity_type == &EntityType::PLAYER,
                    6.0,
                    30,
                    60,
                )),
            ),
            (
                1,
                Box::new(Timed::new(behavior::AnimalMakeLove::new(
                    &EntityType::CAMEL,
                    SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                    2,
                ))),
            ),
            (
                2,
                Box::new(behavior::run_one(vec![
                    (
                        Box::new(Timed::new(behavior::FollowTemptation::with_distance(
                            |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                            |camel| {
                                if is_baby(camel) {
                                    behavior::follow_temptation::DEFAULT_CLOSE_ENOUGH_DIST
                                } else {
                                    behavior::follow_temptation::BACKED_UP_CLOSE_ENOUGH_DIST
                                }
                            },
                            false,
                        ))) as Box<dyn BehaviorControl>,
                        1,
                    ),
                    (
                        unless_refusing_to_move(behavior::baby_follow_adult(
                            ADULT_FOLLOW_RANGE.0,
                            ADULT_FOLLOW_RANGE.1,
                            |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                        )),
                        1,
                    ),
                ])),
            ),
            (
                3,
                Box::new(Timed::new(behavior::RandomLookAround::new(
                    LOOK_AROUND_INTERVAL,
                    30.0,
                    0.0,
                    0.0,
                ))),
            ),
            (
                4,
                Box::new(behavior::run_one_with_conditions(
                    vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
                    vec![
                        (
                            unless_refusing_to_move(behavior::stroll(
                                SPEED_MULTIPLIER_WHEN_IDLING,
                                true,
                            )),
                            1,
                        ),
                        (
                            unless_refusing_to_move(behavior::set_walk_target_from_look_target(
                                |_| true,
                                SPEED_MULTIPLIER_WHEN_IDLING,
                                3,
                            )),
                            1,
                        ),
                        (
                            Box::new(Timed::new(RandomSitting {
                                minimal_pose_ticks: i64::from(MINIMAL_SITTING_POSE_SECONDS * 20),
                            })),
                            1,
                        ),
                        (Box::new(behavior::DoNothing::new(30, 60)), 1),
                    ],
                )),
            ),
        ],
    )
}

/// Vanilla `CamelAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Idle]);
}

/// Vanilla `CamelAi.CamelPanic`: stands up at once before fleeing. Camels are never
/// mob controlled, so the extra start condition always holds.
struct CamelPanic(behavior::AnimalPanic);

impl Behavior for CamelPanic {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        self.0.entry_conditions()
    }

    fn min_duration(&self) -> i32 {
        self.0.min_duration()
    }

    fn max_duration(&self) -> i32 {
        self.0.max_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        self.0.check_extra_start_conditions(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(camel) = as_camel(tick.mob) {
            camel.stand_up_instantly(tick.time);
        }
        self.0.start(tick);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.0.stop(tick);
    }

    fn debug_name(&self) -> &'static str {
        "CamelPanic"
    }
}

/// Vanilla `CamelAi.RandomSitting`: sits down or stands up after holding a pose long enough.
struct RandomSitting {
    minimal_pose_ticks: i64,
}

impl Behavior for RandomSitting {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(camel) = as_camel(tick.mob) else {
            return false;
        };
        let entity = tick.mob.get_entity();
        !entity.is_in_water()
            && camel.get_pose_time(tick.time) >= self.minimal_pose_ticks
            && !entity.is_leashed()
            && entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
            && !entity.has_passengers()
            && camel.can_camel_change_pose(tick.world)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(camel) = as_camel(tick.mob) else {
            return;
        };
        if camel.is_camel_sitting() {
            camel.stand_up(tick.time);
        } else if !tick.brain.has_memory_value(types::IS_PANICKING.id()) {
            camel.sit_down(tick.time);
        }
    }

    fn debug_name(&self) -> &'static str {
        "RandomSitting"
    }
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::HurtBy,
    SensorType::FoodTemptations,
    SensorType::NearestAdult,
];

pub static CAMEL_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| vec![init_core_activity(), init_idle_activity()],
};
