use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use rand::RngExt;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::ai::brain::{
    Brain, BrainProvider, BrainState, BrainTick, VisibilityContext,
    activity::ActivityData,
    behavior::{
        AnimalMakeLove, AnimalPanic, BehaviorControl, ChargeAttack, CountDownCooldownTicks,
        FollowTemptation, GateBehavior, LookAtTargetSink, MoveToTargetSink, OrderPolicy,
        RunningPolicy, Timed, random_stroll, set_walk_target_from_look_target, start_attacking,
        utils,
    },
    memory::{MemoryModuleId, MemoryStatus, PackedMemories, types},
    sensing::{self, SensorType},
};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::mob::Mob;

const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.3;
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.4;
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 1.6;
const TIME_BETWEEN_NON_PLAYER_ATTACKS: (i32, i32) = (2400, 3600);
const SPEED_WHEN_ATTACKING: f32 = 0.6;
const ATTACK_KNOCKBACK_FORCE: f32 = 2.0;
const ANGER_DURATION: i64 = 400;
const TIME_BETWEEN_ATTACKS: i32 = 80;
const MAX_CHARGE_DISTANCE: f64 = 12.0;
const MAX_TARGET_DETECTION_DISTANCE: f64 = 11.0;
const BABY_TEMPT_DISTANCE: f64 = 2.5;
const ADULT_TEMPT_DISTANCE: f64 = 3.5;

const MEMORY_TYPES: &[MemoryModuleId] = &[types::ANGRY_AT.id(), types::ATTACK_TARGET_COOLDOWN.id()];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestAdult,
    SensorType::NearestPlayers,
    SensorType::HurtBy,
    SensorType::NautilusTemptations,
];

pub(super) fn make_brain(mob: &dyn Mob, packed: &PackedMemories) -> Brain {
    BrainProvider {
        memory_types: MEMORY_TYPES,
        sensor_types: SENSOR_TYPES,
        activities,
    }
    .make_brain(mob, packed)
}

fn attack_target_conditions() -> TargetPredicate {
    let mut conditions = TargetPredicate::create_attackable();
    conditions.set_predicate(|target, world| {
        (world.level_info.load().game_rules.mob_griefing
            || target.entity.entity_type != &EntityType::ARMOR_STAND)
            && world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_bounding_box(&target.entity.bounding_box.load())
    });
    conditions
}

pub(super) fn init_memories(brain: &mut BrainState, mob: &dyn Mob) {
    brain.set(types::ATTACK_TARGET_COOLDOWN, sample_attack_cooldown(mob));
}

fn sample_attack_cooldown(mob: &dyn Mob) -> i32 {
    mob.get_random()
        .random_range(TIME_BETWEEN_NON_PLAYER_ATTACKS.0..=TIME_BETWEEN_NON_PLAYER_ATTACKS.1)
}

fn activities(_mob: &dyn Mob) -> Vec<ActivityData> {
    vec![
        core_activity(),
        ActivityData::with_pairs(
            Activity::Idle,
            vec![
                (
                    1,
                    Box::new(Timed::new(AnimalMakeLove::with_speed(
                        &EntityType::NAUTILUS,
                        SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                        2,
                    ))),
                ),
                (2, follow_temptation()),
                (3, start_attacking()),
                (4, idle_movement()),
            ],
        ),
        fight_activity(),
    ]
}

fn core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(AnimalPanic::new(
                SPEED_MULTIPLIER_WHEN_PANICKING,
            ))),
            Box::new(Timed::new(LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(MoveToTargetSink::default())),
            Box::new(CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            )),
            Box::new(CountDownCooldownTicks::new(types::CHARGE_COOLDOWN_TICKS)),
            Box::new(CountDownCooldownTicks::new(types::ATTACK_TARGET_COOLDOWN)),
        ],
    )
}

fn follow_temptation() -> Box<dyn BehaviorControl> {
    Box::new(Timed::new(FollowTemptation::new(
        |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
        |mob| {
            if mob.get_mob_entity().living_entity.is_baby() {
                BABY_TEMPT_DISTANCE
            } else {
                ADULT_TEMPT_DISTANCE
            }
        },
    )))
}

fn start_attacking() -> Box<dyn BehaviorControl> {
    Box::new(start_attacking::create(
        |_| true,
        find_nearest_valid_attack_target,
    ))
}

fn idle_movement() -> Box<dyn BehaviorControl> {
    Box::new(GateBehavior::new(
        "GateBehavior",
        vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
        Vec::new(),
        OrderPolicy::Ordered,
        RunningPolicy::TryAll,
        vec![
            (
                Box::new(random_stroll::swim(SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER)),
                2,
            ),
            (
                Box::new(set_walk_target_from_look_target::create(
                    SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
                    3,
                )),
                3,
            ),
        ],
    ))
}

fn fight_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Fight,
        vec![(
            0,
            Box::new(Timed::new(ChargeAttack::new(
                TIME_BETWEEN_ATTACKS,
                attack_target_conditions(),
                SPEED_WHEN_ATTACKING,
                ATTACK_KNOCKBACK_FORCE,
                MAX_CHARGE_DISTANCE,
                MAX_TARGET_DETECTION_DISTANCE,
                Sound::EntityNautilusDash,
            ))),
        )],
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (types::CHARGE_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
        ],
    )
}

fn find_nearest_valid_attack_target(tick: &mut BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let mob = tick.mob;
    if utils::is_breeding(tick.state)
        || !mob.get_entity().is_in_water()
        || mob.get_mob_entity().living_entity.is_baby()
        || mob.is_tamed()
    {
        return None;
    }
    let angry_at =
        utils::get_living_entity_from_uuid_memory(tick.state, tick.world, types::ANGRY_AT).filter(
            |entity| {
                entity.get_entity().is_in_water()
                    && sensing::is_entity_attackable_ignoring_line_of_sight(
                        &tick.visibility(),
                        entity.as_ref(),
                    )
            },
        );
    if angry_at.is_some() {
        return angry_at;
    }
    if tick
        .state
        .has_memory_value(types::ATTACK_TARGET_COOLDOWN.id())
    {
        return None;
    }
    tick.state
        .set(types::ATTACK_TARGET_COOLDOWN, sample_attack_cooldown(mob));
    if mob.get_random().random::<f32>() < 0.5 {
        return None;
    }
    let visibility = tick.visibility();
    tick.state
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?
        .find_closest(&visibility, |entity| {
            entity.get_entity().is_in_water()
                && entity
                    .get_entity()
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_NAUTILUS_HOSTILES)
        })
}

pub(super) fn set_anger_target(
    brain: &mut BrainState,
    mob: &dyn Mob,
    target: &dyn EntityBase,
    target_uuid: Uuid,
) {
    let world = mob.get_entity().world.load_full();
    let visibility = VisibilityContext {
        state: brain,
        world: &world,
        mob,
        follow_range: mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&pumpkin_data::attributes::Attributes::FOLLOW_RANGE),
    };
    if sensing::is_entity_attackable_ignoring_line_of_sight(&visibility, target) {
        brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
        brain.set_with_expiry(types::ANGRY_AT, target_uuid, ANGER_DURATION);
    }
}

pub(super) fn update_activity(brain: &mut BrainState) {
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}
