use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::behavior::{self, Behavior, BehaviorControl, Timed};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::mob::Mob;
use crate::entity::mob::sounds;
use crate::entity::passive::armadillo::{ArmadilloEntity, ArmadilloState};

const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 1.0;
const DEFAULT_CLOSE_ENOUGH_DIST: f64 = 2.0;
const BABY_CLOSE_ENOUGH_DIST: f64 = 1.0;
const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
const LOOK_AROUND_INTERVAL: (i32, i32) = (150, 250);
const SWIM_CHANCE: f32 = 0.8;
/// `SensorType.ARMADILLO_SCARE_DETECTED` values.
pub const SCARE_SCAN_RATE: i32 = 5;
pub const SCARE_MEMORY_TIME_TO_LIVE: i64 = 80;

fn as_armadillo(mob: &dyn Mob) -> Option<&ArmadilloEntity> {
    mob.cast_any().downcast_ref::<ArmadilloEntity>()
}

fn is_scared(mob: &dyn Mob) -> bool {
    as_armadillo(mob).is_some_and(ArmadilloEntity::is_scared)
}

/// Vanilla `Armadillo.isScaredBy`, for the scare sensor.
pub fn is_scared_by(mob: &dyn Mob, entity: &dyn EntityBase) -> bool {
    as_armadillo(mob).is_some_and(|armadillo| armadillo.is_scared_by(entity))
}

/// Vanilla `Armadillo.canStayRolledUp`, for the scare sensor.
pub fn can_stay_rolled_up(mob: &dyn Mob) -> bool {
    as_armadillo(mob).is_some_and(ArmadilloEntity::can_stay_rolled_up)
}

/// Vanilla `ArmadilloAi.ARMADILLO_ROLLING_OUT`.
fn armadillo_rolling_out() -> OneShot {
    OneShot::new(
        "ArmadilloRollingOut",
        vec![(
            types::DANGER_DETECTED_RECENTLY.id(),
            MemoryStatus::ValueAbsent,
        )],
        |tick| {
            let Some(armadillo) = as_armadillo(tick.mob) else {
                return false;
            };
            if armadillo.is_scared() {
                armadillo.roll_out();
                true
            } else {
                false
            }
        },
    )
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::new(SWIM_CHANCE))),
            Box::new(Timed::new(ArmadilloPanic(
                behavior::AnimalPanic::with_causes(SPEED_MULTIPLIER_WHEN_PANICKING, |_| {
                    &tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES
                }),
            ))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(ArmadilloMoveToTargetSink(
                behavior::MoveToTargetSink::default(),
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::GAZE_COOLDOWN_TICKS,
            ))),
            Box::new(armadillo_rolling_out()),
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
                    &EntityType::ARMADILLO,
                    SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                    1,
                ))),
            ),
            (
                2,
                Box::new(behavior::run_one(vec![
                    (
                        Box::new(Timed::new(behavior::FollowTemptation::with_distance(
                            |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                            |armadillo| {
                                if is_baby(armadillo) {
                                    BABY_CLOSE_ENOUGH_DIST
                                } else {
                                    DEFAULT_CLOSE_ENOUGH_DIST
                                }
                            },
                            false,
                        ))) as Box<dyn BehaviorControl>,
                        1,
                    ),
                    (
                        Box::new(behavior::baby_follow_adult(
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
                            Box::new(behavior::stroll(SPEED_MULTIPLIER_WHEN_IDLING, true)),
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
                        (Box::new(behavior::DoNothing::new(30, 60)), 1),
                    ],
                )),
            ),
        ],
    )
}

fn init_scared_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Panic,
        vec![(0, Box::new(Timed::new(ArmadilloBallUp::default())))],
        vec![
            (
                types::DANGER_DETECTED_RECENTLY.id(),
                MemoryStatus::ValuePresent,
            ),
            (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
        ],
    )
}

/// Vanilla `ArmadilloAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Panic, Activity::Idle]);
}

/// Vanilla `ArmadilloAi.ArmadilloPanic`: panics only from environmental damage, unrolling first.
struct ArmadilloPanic(behavior::AnimalPanic);

impl Behavior for ArmadilloPanic {
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
        if let Some(armadillo) = as_armadillo(tick.mob) {
            armadillo.roll_out();
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
        "ArmadilloPanic"
    }
}

/// Vanilla's anonymous `MoveToTargetSink` that a rolled-up armadillo never starts.
struct ArmadilloMoveToTargetSink(behavior::MoveToTargetSink);

impl Behavior for ArmadilloMoveToTargetSink {
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
        !is_scared(tick.mob) && self.0.check_extra_start_conditions(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        self.0.can_still_use(tick)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.0.start(tick);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.0.stop(tick);
    }

    fn debug_name(&self) -> &'static str {
        "MoveToTargetSink"
    }
}

const BALL_UP_STAY_IN_STATE: i32 = 5 * 60 * 20;
const DANGER_DETECTED_RECENTLY_DANGER_THRESHOLD: i64 = 75;
const PEEK_DELAY: (i32, i32) = (100, 400);

/// Vanilla `ArmadilloAi.ArmadilloBallUp`: rolls up and peeks out until the danger passes.
#[derive(Default)]
struct ArmadilloBallUp {
    next_peek_timer: i32,
    danger_was_around: bool,
}

impl ArmadilloBallUp {
    fn pick_next_peek_timer(mob: &dyn Mob) -> i32 {
        ArmadilloState::Scared.animation_duration() as i32
            + mob.get_random().random_range(PEEK_DELAY.0..=PEEK_DELAY.1)
    }
}

impl Behavior for ArmadilloBallUp {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &[]
    }

    fn min_duration(&self) -> i32 {
        BALL_UP_STAY_IN_STATE
    }

    fn max_duration(&self) -> i32 {
        BALL_UP_STAY_IN_STATE
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
            && !entity.is_in_water()
            && !entity
                .touching_lava
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        as_armadillo(tick.mob).is_some_and(|armadillo| armadillo.get_state().is_threatened())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(armadillo) = as_armadillo(tick.mob) {
            armadillo.roll_up();
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(armadillo) = as_armadillo(tick.mob) else {
            return;
        };
        if self.next_peek_timer > 0 {
            self.next_peek_timer -= 1;
        }
        let on_ground = tick
            .mob
            .get_entity()
            .on_ground
            .load(std::sync::atomic::Ordering::Relaxed);
        if armadillo.should_switch_to_scared_state() {
            armadillo.switch_to_state(ArmadilloState::Scared);
            if on_ground {
                sounds::make_sound(tick.mob, Sound::EntityArmadilloLand, SoundCategory::Neutral);
            }
            return;
        }
        let state = armadillo.get_state();
        let danger_tick_counter = tick
            .brain
            .time_until_expiry(types::DANGER_DETECTED_RECENTLY.id())
            .unwrap_or(0);
        let danger_is_around = danger_tick_counter > DANGER_DETECTED_RECENTLY_DANGER_THRESHOLD;
        if danger_is_around != self.danger_was_around {
            self.next_peek_timer = Self::pick_next_peek_timer(tick.mob);
        }
        self.danger_was_around = danger_is_around;
        let unrolling_duration = ArmadilloState::Unrolling.animation_duration() as i64;
        if state == ArmadilloState::Scared {
            if self.next_peek_timer == 0 && on_ground && danger_is_around {
                tick.world.send_entity_status(
                    tick.mob.get_entity(),
                    EntityStatus::ArmadilloPeek,
                    None,
                );
                self.next_peek_timer = Self::pick_next_peek_timer(tick.mob);
            }
            if danger_tick_counter < unrolling_duration {
                sounds::make_sound(
                    tick.mob,
                    Sound::EntityArmadilloUnrollStart,
                    SoundCategory::Neutral,
                );
                armadillo.switch_to_state(ArmadilloState::Unrolling);
            }
        } else if state == ArmadilloState::Unrolling && danger_tick_counter > unrolling_duration {
            armadillo.switch_to_state(ArmadilloState::Scared);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(armadillo) = as_armadillo(tick.mob)
            && !armadillo.can_stay_rolled_up()
        {
            armadillo.roll_out();
        }
    }

    fn debug_name(&self) -> &'static str {
        "ArmadilloBallUp"
    }
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::HurtBy,
    SensorType::FoodTemptations,
    SensorType::NearestAdult,
    SensorType::ArmadilloScareDetected,
];

pub static ARMADILLO_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_scared_activity(),
        ]
    },
};
