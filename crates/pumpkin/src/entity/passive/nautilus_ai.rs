use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::utils::{get_living_entity_from_uuid_memory, is_breeding};
use crate::entity::ai::brain::behavior::{self, GateBehavior, OrderPolicy, RunningPolicy, Timed};
use crate::entity::ai::brain::memory::{
    MemoryModuleId, MemoryStatus, NearestVisibleLivingEntities, types,
};
use crate::entity::ai::brain::sensing::{SensorType, is_entity_attackable_ignoring_line_of_sight};
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::living::LivingEntity;
use crate::entity::mob::Mob;
use crate::entity::passive::nautilus::NautilusEntity;
use crate::world::World;

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
const BABY_CLOSE_ENOUGH_DIST: f64 = 2.5;
const ADULT_CLOSE_ENOUGH_DIST: f64 = 3.5;
const NON_PLAYER_ATTACK_CHANCE: f32 = 0.5;

fn as_nautilus(mob: &dyn Mob) -> Option<&NautilusEntity> {
    mob.cast_any().downcast_ref::<NautilusEntity>()
}

fn is_tame(mob: &dyn Mob) -> bool {
    as_nautilus(mob).is_some_and(NautilusEntity::is_tame)
}

/// Vanilla `NautilusAi.getTemptations`.
#[must_use]
pub fn is_temptation(_mob: &dyn Mob, stack: &ItemStack) -> bool {
    stack.item.has_tag(&tag::Item::MINECRAFT_NAUTILUS_FOOD)
}

/// Vanilla `NautilusAi.ATTACK_TARGET_CONDITIONS`.
fn attack_target_conditions() -> TargetPredicate {
    let mut conditions = TargetPredicate::create_attackable();
    conditions.set_predicate(|target: &LivingEntity, world: &World| {
        let bounds = target.entity.bounding_box.load();
        let within_border = {
            let border = world
                .worldborder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            border.contains(bounds.min.x, bounds.min.z)
                && border.contains(bounds.max.x - 1.0E-5, bounds.max.z - 1.0E-5)
        };
        (world.level_info.load().game_rules.mob_griefing
            || target.entity.entity_type != &EntityType::ARMOR_STAND)
            && within_border
    });
    conditions
}

/// Vanilla `NautilusAi.initMemories`.
pub fn init_memories(brain: &mut Brain, mob: &dyn Mob) {
    let cooldown = mob
        .get_random()
        .random_range(TIME_BETWEEN_NON_PLAYER_ATTACKS.0..=TIME_BETWEEN_NON_PLAYER_ATTACKS.1);
    brain.set(types::ATTACK_TARGET_COOLDOWN, cooldown);
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
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::CHARGE_COOLDOWN_TICKS,
            ))),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::ATTACK_TARGET_COOLDOWN,
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
                Box::new(Timed::new(behavior::AnimalMakeLove::new(
                    &EntityType::NAUTILUS,
                    SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                    2,
                ))),
            ),
            (
                2,
                Box::new(Timed::new(behavior::FollowTemptation::with_distance(
                    |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                    |nautilus| {
                        if is_baby(nautilus) {
                            BABY_CLOSE_ENOUGH_DIST
                        } else {
                            ADULT_CLOSE_ENOUGH_DIST
                        }
                    },
                    false,
                ))),
            ),
            (
                3,
                Box::new(behavior::start_attacking(
                    |_| true,
                    find_nearest_valid_attack_target,
                )),
            ),
            (
                4,
                Box::new(GateBehavior::new(
                    "NautilusIdleMovement",
                    vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
                    Vec::new(),
                    OrderPolicy::Ordered,
                    RunningPolicy::TryAll,
                    vec![
                        (
                            Box::new(behavior::swim(SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER)),
                            2,
                        ),
                        (
                            Box::new(behavior::set_walk_target_from_look_target(
                                |_| true,
                                SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
                                3,
                            )),
                            3,
                        ),
                    ],
                )),
            ),
        ],
    )
}

fn init_fight_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Fight,
        vec![(
            0,
            Box::new(Timed::new(behavior::ChargeAttack::new(
                TIME_BETWEEN_ATTACKS,
                attack_target_conditions(),
                SPEED_WHEN_ATTACKING,
                ATTACK_KNOCKBACK_FORCE,
                MAX_CHARGE_DISTANCE,
                MAX_TARGET_DETECTION_DISTANCE,
                Sound::EntityNautilusDash,
                is_tame,
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

/// Vanilla `NautilusAi.findNearestValidAttackTarget`.
fn find_nearest_valid_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let body = tick.mob;
    if is_breeding(tick.brain) || !body.get_entity().is_in_water() || is_baby(body) || is_tame(body)
    {
        return None;
    }
    let ctx = tick.visibility();
    let angry_at = get_living_entity_from_uuid_memory(tick.brain, tick.world, types::ANGRY_AT)
        .filter(|entity| {
            entity.get_entity().is_in_water()
                && is_entity_attackable_ignoring_line_of_sight(&ctx, entity.as_ref())
        });
    if angry_at.is_some() {
        return angry_at;
    }
    if tick
        .brain
        .has_memory_value(types::ATTACK_TARGET_COOLDOWN.id())
    {
        return None;
    }
    // Vanilla resets this cooldown inside the target lookup; it lands through the inbox.
    let cooldown = body
        .get_random()
        .random_range(TIME_BETWEEN_NON_PLAYER_ATTACKS.0..=TIME_BETWEEN_NON_PLAYER_ATTACKS.1);
    if let Some(mob_entity) = body.as_mob_entity() {
        mob_entity.post_to_brain(Box::new(move |tick| {
            tick.brain.set(types::ATTACK_TARGET_COOLDOWN, cooldown);
        }));
    }
    if body.get_random().random::<f32>() < NON_PLAYER_ATTACK_CHANCE {
        return None;
    }
    let empty = NearestVisibleLivingEntities::empty();
    ctx.brain
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
        .unwrap_or(&empty)
        .find_closest(&ctx, |entity| {
            entity.get_entity().is_in_water()
                && entity
                    .get_entity()
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_NAUTILUS_HOSTILES)
        })
}

/// Vanilla `NautilusAi.setAngerTarget`.
pub fn set_anger_target(tick: &mut BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    if is_entity_attackable_ignoring_line_of_sight(&tick.visibility(), target.as_ref()) {
        tick.brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
        tick.brain.set_with_expiry(
            types::ANGRY_AT,
            target.get_entity().entity_uuid,
            ANGER_DURATION,
        );
    }
}

/// Vanilla `NautilusAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestAdult,
    SensorType::NearestPlayers,
    SensorType::HurtBy,
    SensorType::NautilusTemptations,
];

pub static NAUTILUS_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_fight_activity(),
        ]
    },
};
