use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use rand::RngExt;

use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::{self, Timed};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::passive::goat::GoatEntity;
use crate::world::World;

pub const RAM_PREPARE_TIME: i64 = 20;
pub const RAM_MAX_DISTANCE: i32 = 7;
pub const RAM_MIN_DISTANCE: i32 = 4;
pub const MAX_LONG_JUMP_HEIGHT: i32 = 5;
pub const MAX_LONG_JUMP_WIDTH: i32 = 5;
pub const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;
pub const ADULT_RAM_KNOCKBACK_FORCE: f64 = 2.5;
pub const BABY_RAM_KNOCKBACK_FORCE: f64 = 1.0;
const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
const SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM: f32 = 1.25;
const SPEED_MULTIPLIER_WHEN_RAMMING: f32 = 3.0;
const TIME_BETWEEN_LONG_JUMPS: (i32, i32) = (600, 1200);
const TIME_BETWEEN_RAMS: (i32, i32) = (600, 6000);
const TIME_BETWEEN_RAMS_SCREAMER: (i32, i32) = (100, 300);
const SWIM_CHANCE: f32 = 0.8;
const LONG_JUMP_DIMENSIONS_SCALE: f32 = 0.7;

fn is_screaming(mob: &dyn Mob) -> bool {
    mob.cast_any()
        .downcast_ref::<GoatEntity>()
        .is_some_and(GoatEntity::is_screaming)
}

fn time_between_rams(mob: &dyn Mob) -> (i32, i32) {
    if is_screaming(mob) {
        TIME_BETWEEN_RAMS_SCREAMER
    } else {
        TIME_BETWEEN_RAMS
    }
}

/// Vanilla `GoatAi.RAM_TARGET_CONDITIONS`.
fn ram_target_conditions() -> TargetPredicate {
    let mut conditions = TargetPredicate::create_attackable();
    conditions.set_predicate(|target: &LivingEntity, world: &World| {
        let target_type = target.entity.entity_type;
        let bounds = target.entity.bounding_box.load();
        let within_border = {
            let border = world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            border.contains(bounds.min.x, bounds.min.z)
                && border.contains(bounds.max.x - 1.0E-5, bounds.max.z - 1.0E-5)
        };
        target_type != &EntityType::GOAT
            && (world.level_info.load().game_rules.mob_griefing
                || target_type != &EntityType::ARMOR_STAND)
            && within_border
    });
    conditions
}

/// Vanilla `GoatAi.initMemories`.
pub fn init_memories(brain: &mut Brain, mob: &dyn Mob) {
    let mut rng = mob.get_random();
    brain.set(
        types::LONG_JUMP_COOLDOWN_TICKS,
        rng.random_range(TIME_BETWEEN_LONG_JUMPS.0..=TIME_BETWEEN_LONG_JUMPS.1),
    );
    brain.set(
        types::RAM_COOLDOWN_TICKS,
        rng.random_range(TIME_BETWEEN_RAMS.0..=TIME_BETWEEN_RAMS.1),
    );
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::new(SWIM_CHANCE))),
            Box::new(Timed::new(behavior::AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::LONG_JUMP_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::RAM_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
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
                0,
                Box::new(Timed::new(behavior::AnimalMakeLove::new(
                    &EntityType::GOAT,
                    1.0,
                    2,
                ))),
            ),
            (
                1,
                Box::new(Timed::new(behavior::FollowTemptation::new(|_| {
                    SPEED_MULTIPLIER_WHEN_TEMPTED
                }))),
            ),
            (
                2,
                Box::new(behavior::baby_follow_adult(
                    ADULT_FOLLOW_RANGE.0,
                    ADULT_FOLLOW_RANGE.1,
                    |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                )),
            ),
            (
                3,
                Box::new(behavior::run_one(vec![
                    (
                        Box::new(behavior::stroll(SPEED_MULTIPLIER_WHEN_IDLING, true)),
                        2,
                    ),
                    (
                        Box::new(behavior::set_walk_target_from_look_target(
                            |_| true,
                            SPEED_MULTIPLIER_WHEN_IDLING,
                            3,
                        )),
                        2,
                    ),
                    (Box::new(behavior::DoNothing::new(30, 60)), 1),
                ])),
            ),
        ],
        vec![
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValuePresent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValuePresent,
            ),
        ],
    )
}

fn init_long_jump_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::LongJump,
        vec![
            (
                0,
                Box::new(Timed::new(behavior::LongJumpMidJump::new(
                    TIME_BETWEEN_LONG_JUMPS,
                    Sound::EntityGoatStep,
                ))),
            ),
            (
                1,
                Box::new(Timed::new(
                    behavior::LongJumpToRandomPos::new(
                        TIME_BETWEEN_LONG_JUMPS,
                        MAX_LONG_JUMP_HEIGHT,
                        MAX_LONG_JUMP_WIDTH,
                        MAX_JUMP_VELOCITY_MULTIPLIER,
                        |goat| {
                            if is_screaming(goat) {
                                Sound::EntityGoatScreamingLongJump
                            } else {
                                Sound::EntityGoatLongJump
                            }
                        },
                        behavior::default_acceptable_landing_spot,
                    )
                    .with_jump_dimensions_scale(LONG_JUMP_DIMENSIONS_SCALE),
                )),
            ),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValueAbsent,
            ),
        ],
    )
}

fn init_ram_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Ram,
        vec![
            (
                0,
                Box::new(Timed::new(behavior::RamTarget::new(
                    time_between_rams,
                    ram_target_conditions(),
                    SPEED_MULTIPLIER_WHEN_RAMMING,
                    |goat| {
                        if is_baby(goat) {
                            BABY_RAM_KNOCKBACK_FORCE
                        } else {
                            ADULT_RAM_KNOCKBACK_FORCE
                        }
                    },
                    |goat| {
                        if is_screaming(goat) {
                            Sound::EntityGoatScreamingRamImpact
                        } else {
                            Sound::EntityGoatRamImpact
                        }
                    },
                    |_| Sound::EntityGoatHornBreak,
                    |goat| {
                        goat.cast_any()
                            .downcast_ref::<GoatEntity>()
                            .is_some_and(GoatEntity::drop_horn)
                    },
                ))),
            ),
            (
                1,
                Box::new(Timed::new(behavior::PrepareRamNearestTarget::new(
                    |goat| time_between_rams(goat).0,
                    RAM_MIN_DISTANCE,
                    RAM_MAX_DISTANCE,
                    SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM,
                    ram_target_conditions(),
                    RAM_PREPARE_TIME,
                    |goat| {
                        if is_screaming(goat) {
                            Sound::EntityGoatScreamingPrepareRam
                        } else {
                            Sound::EntityGoatPrepareRam
                        }
                    },
                ))),
            ),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
        ],
    )
}

/// Vanilla `GoatAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain.set_active_activity_to_first_valid(&[
        Activity::Ram,
        Activity::LongJump,
        Activity::Idle,
    ]);
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestPlayers,
    SensorType::NearestItems,
    SensorType::NearestAdult,
    SensorType::HurtBy,
    SensorType::FoodTemptations,
];

pub static GOAT_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_long_jump_activity(),
            init_ram_activity(),
        ]
    },
};
