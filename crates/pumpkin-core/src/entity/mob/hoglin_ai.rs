use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::position::BlockPos;

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::one_shot::sequence;
use crate::entity::ai::brain::behavior::utils::{
    get_nearest_target, is_breeding, is_other_target_much_further_away_than_current_attack_target,
};
use crate::entity::ai::brain::behavior::{self, BehaviorControl};
use crate::entity::ai::brain::memory::{MemoryModuleId, types};
use crate::entity::ai::brain::sensing::{SensorType, is_entity_attackable};
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::mob::piglin_ai::{RETREAT_DURATION, as_hoglin, sample};

pub const REPELLENT_DETECTION_RANGE_HORIZONTAL: i32 = 8;
pub const REPELLENT_DETECTION_RANGE_VERTICAL: i32 = 4;
const ATTACK_DURATION: i64 = 200;
const DESIRED_DISTANCE_FROM_PIGLIN_WHEN_IDLING: i32 = 8;
const DESIRED_DISTANCE_FROM_PIGLIN_WHEN_RETREATING: i32 = 15;
const ATTACK_INTERVAL: i32 = 40;
const BABY_ATTACK_INTERVAL: i32 = 15;
const REPELLENT_PACIFY_TIME: i64 = 200;
const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
const SPEED_MULTIPLIER_WHEN_AVOIDING_REPELLENT: f32 = 1.0;
const SPEED_MULTIPLIER_WHEN_RETREATING: f32 = 1.3;
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.6;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.4;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 0.6;
const MAX_LOOK_DIST: f32 = 8.0;
const LOOK_INTERVAL: (i32, i32) = (30, 60);

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(behavior::Timed::new(behavior::LookAtTargetSink::new(
                45, 90,
            ))),
            Box::new(behavior::Timed::new(behavior::MoveToTargetSink::default())),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        10,
        vec![
            Box::new(behavior::become_passive_if_memory_present(
                types::NEAREST_REPELLENT.id(),
                REPELLENT_PACIFY_TIME,
            )),
            Box::new(behavior::Timed::new(behavior::AnimalMakeLove::new(
                &EntityType::HOGLIN,
                SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                2,
            ))),
            Box::new(behavior::set_walk_target_away_from_pos(
                types::NEAREST_REPELLENT,
                SPEED_MULTIPLIER_WHEN_AVOIDING_REPELLENT,
                REPELLENT_DETECTION_RANGE_HORIZONTAL,
                true,
            )),
            Box::new(behavior::start_attacking(
                |_| true,
                find_nearest_valid_attack_target,
            )),
            Box::new(sequence(
                "TriggerIf(isAdult)",
                |tick| !is_baby(tick.mob),
                behavior::set_walk_target_away_from_entity(
                    types::NEAREST_VISIBLE_ADULT_PIGLIN,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    DESIRED_DISTANCE_FROM_PIGLIN_WHEN_IDLING,
                    false,
                ),
            )),
            Box::new(behavior::set_entity_look_target_sometimes(
                |_| true,
                MAX_LOOK_DIST,
                LOOK_INTERVAL.0,
                LOOK_INTERVAL.1,
            )),
            Box::new(behavior::baby_follow_adult(
                ADULT_FOLLOW_RANGE.0,
                ADULT_FOLLOW_RANGE.1,
                |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
            )),
            idle_movement_behaviors(),
        ],
    )
}

fn init_fight_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Fight,
        10,
        vec![
            Box::new(behavior::become_passive_if_memory_present(
                types::NEAREST_REPELLENT.id(),
                REPELLENT_PACIFY_TIME,
            )),
            Box::new(behavior::Timed::new(behavior::AnimalMakeLove::new(
                &EntityType::HOGLIN,
                SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                2,
            ))),
            Box::new(behavior::set_walk_target_from_attack_target_if_target_out_of_reach(1.0)),
            Box::new(behavior::melee_attack(
                |tick| !is_baby(tick.mob),
                ATTACK_INTERVAL,
            )),
            Box::new(behavior::melee_attack(
                |tick| is_baby(tick.mob),
                BABY_ATTACK_INTERVAL,
            )),
            Box::new(behavior::stop_attacking_if_target_invalid(
                |_, _| false,
                |_, _| {},
                true,
            )),
            Box::new(behavior::erase_memory_if(
                "EraseMemoryIf(isBreeding)",
                |tick| is_breeding(tick.brain),
                types::ATTACK_TARGET.id(),
            )),
        ],
        types::ATTACK_TARGET.id(),
    )
}

fn init_retreat_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Avoid,
        10,
        vec![
            Box::new(behavior::set_walk_target_away_from_entity(
                types::AVOID_TARGET,
                SPEED_MULTIPLIER_WHEN_RETREATING,
                DESIRED_DISTANCE_FROM_PIGLIN_WHEN_RETREATING,
                false,
            )),
            idle_movement_behaviors(),
            Box::new(behavior::set_entity_look_target_sometimes(
                |_| true,
                MAX_LOOK_DIST,
                LOOK_INTERVAL.0,
                LOOK_INTERVAL.1,
            )),
            Box::new(behavior::erase_memory_if(
                "EraseMemoryIf(wantsToStopFleeing)",
                wants_to_stop_fleeing,
                types::AVOID_TARGET.id(),
            )),
        ],
        types::AVOID_TARGET.id(),
    )
}

fn idle_movement_behaviors() -> Box<dyn BehaviorControl> {
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
    ]))
}

/// Vanilla `HoglinAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    let old_activity = tick.brain.get_active_non_core_activity();
    tick.brain.set_active_activity_to_first_valid(&[
        Activity::Fight,
        Activity::Avoid,
        Activity::Idle,
    ]);
    let new_activity = tick.brain.get_active_non_core_activity();
    if old_activity != new_activity
        && let Some(sound) = get_sound_for_current_activity(tick)
        && let Some(hoglin) = as_hoglin(tick.mob)
    {
        hoglin.make_sound(sound);
    }
    let has_attack_target = tick.brain.has_memory_value(types::ATTACK_TARGET.id());
    tick.mob.get_mob_entity().set_attacking(has_attack_target);
}

/// Vanilla `HoglinAi.onHitTarget`.
pub fn on_hit_target(tick: &mut BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    if is_baby(tick.mob) {
        return;
    }
    if target.get_entity().entity_type == &EntityType::PIGLIN && piglins_outnumber_hoglins(tick) {
        set_avoid_target(tick, Arc::clone(target));
        broadcast_retreat(tick, target);
    } else {
        broadcast_attack_target(tick, target);
    }
}

fn broadcast_retreat(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    for hoglin in get_visible_adult_hoglins(tick.brain) {
        let Some(mob_entity) = hoglin.as_mob_entity() else {
            continue;
        };
        let target = Arc::clone(target);
        mob_entity.post_to_brain(Box::new(move |tick| {
            retreat_from_nearest_target(tick, target);
        }));
    }
}

fn retreat_from_nearest_target(tick: &mut BrainTick<'_>, new_avoid_target: Arc<dyn EntityBase>) {
    let mut nearest = new_avoid_target;
    nearest = get_nearest_target(
        tick.mob,
        tick.brain.get(types::AVOID_TARGET).cloned(),
        nearest,
    );
    nearest = get_nearest_target(
        tick.mob,
        tick.brain.get(types::ATTACK_TARGET).cloned(),
        nearest,
    );
    set_avoid_target(tick, nearest);
}

fn set_avoid_target(tick: &mut BrainTick<'_>, avoid_target: Arc<dyn EntityBase>) {
    tick.brain.erase(types::ATTACK_TARGET.id());
    tick.brain.erase(types::WALK_TARGET.id());
    let duration = sample(RETREAT_DURATION, &mut tick.mob.get_random());
    tick.brain
        .set_with_expiry(types::AVOID_TARGET, avoid_target, i64::from(duration));
}

fn find_nearest_valid_attack_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    if is_pacified(tick.brain) || is_breeding(tick.brain) {
        return None;
    }
    tick.brain
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
        .map(|player| Arc::clone(player) as Arc<dyn EntityBase>)
}

/// Vanilla `HoglinAi.isPosNearNearestRepellent`, given the `NEAREST_REPELLENT` memory.
#[must_use]
pub fn is_pos_near_nearest_repellent(nearest_repellent: Option<BlockPos>, pos: &BlockPos) -> bool {
    let range = f64::from(REPELLENT_DETECTION_RANGE_HORIZONTAL);
    nearest_repellent
        .is_some_and(|repellent| f64::from(repellent.squared_distance(pos)) < range * range)
}

fn wants_to_stop_fleeing(tick: &BrainTick<'_>) -> bool {
    !is_baby(tick.mob) && !piglins_outnumber_hoglins(tick)
}

fn piglins_outnumber_hoglins(tick: &BrainTick<'_>) -> bool {
    if is_baby(tick.mob) {
        return false;
    }
    let piglin_count = tick
        .brain
        .get(types::VISIBLE_ADULT_PIGLIN_COUNT)
        .copied()
        .unwrap_or(0);
    let hoglin_count = tick
        .brain
        .get(types::VISIBLE_ADULT_HOGLIN_COUNT)
        .copied()
        .unwrap_or(0)
        + 1;
    piglin_count > hoglin_count
}

/// Vanilla `HoglinAi.wasHurtBy`.
pub fn was_hurt_by(tick: &mut BrainTick<'_>, attacker: &Arc<dyn EntityBase>) {
    tick.brain.erase(types::PACIFIED.id());
    tick.brain.erase(types::BREED_TARGET.id());
    if is_baby(tick.mob) {
        retreat_from_nearest_target(tick, Arc::clone(attacker));
    } else {
        maybe_retaliate(tick, attacker);
    }
}

fn maybe_retaliate(tick: &mut BrainTick<'_>, attacker: &Arc<dyn EntityBase>) {
    let attacker_type = attacker.get_entity().entity_type;
    if tick.brain.active_activities().contains(Activity::Avoid)
        && attacker_type == &EntityType::PIGLIN
    {
        return;
    }
    if attacker_type == &EntityType::HOGLIN {
        return;
    }
    if is_other_target_much_further_away_than_current_attack_target(
        tick.brain,
        tick.mob,
        attacker.as_ref(),
        4.0,
    ) {
        return;
    }
    if !is_entity_attackable(&tick.visibility(), attacker.as_ref()) {
        return;
    }
    set_attack_target(tick.brain, Arc::clone(attacker));
    broadcast_attack_target(tick, attacker);
}

fn set_attack_target(brain: &mut Brain, target: Arc<dyn EntityBase>) {
    brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
    brain.erase(types::BREED_TARGET.id());
    brain.set_with_expiry(types::ATTACK_TARGET, target, ATTACK_DURATION);
}

fn broadcast_attack_target(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    for hoglin in get_visible_adult_hoglins(tick.brain) {
        let Some(mob_entity) = hoglin.as_mob_entity() else {
            continue;
        };
        let target = Arc::clone(target);
        mob_entity.post_to_brain(Box::new(move |tick| {
            set_attack_target_if_closer_than_current(tick, target);
        }));
    }
}

fn set_attack_target_if_closer_than_current(
    tick: &mut BrainTick<'_>,
    new_target: Arc<dyn EntityBase>,
) {
    if is_pacified(tick.brain) {
        return;
    }
    let current = tick.brain.get(types::ATTACK_TARGET).cloned();
    let nearest = get_nearest_target(tick.mob, current, new_target);
    set_attack_target(tick.brain, nearest);
}

/// Vanilla `HoglinAi.getSoundForCurrentActivity`.
#[must_use]
pub fn get_sound_for_current_activity(tick: &BrainTick<'_>) -> Option<Sound> {
    let activity = tick.brain.get_active_non_core_activity()?;
    let converting = as_hoglin(tick.mob).is_some_and(|hoglin| hoglin.is_converting(tick.world));
    Some(if activity == Activity::Avoid || converting {
        Sound::EntityHoglinRetreat
    } else if activity == Activity::Fight {
        Sound::EntityHoglinAngry
    } else if is_near_repellent(tick.brain) {
        Sound::EntityHoglinRetreat
    } else {
        Sound::EntityHoglinAmbient
    })
}

fn get_visible_adult_hoglins(brain: &Brain) -> Vec<Arc<dyn EntityBase>> {
    brain
        .get(types::NEAREST_VISIBLE_ADULT_HOGLINS)
        .cloned()
        .unwrap_or_default()
}

fn is_near_repellent(brain: &Brain) -> bool {
    brain.has_memory_value(types::NEAREST_REPELLENT.id())
}

/// Vanilla `HoglinAi.isPacified`.
#[must_use]
pub fn is_pacified(brain: &Brain) -> bool {
    brain.has_memory_value(types::PACIFIED.id())
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestPlayers,
    SensorType::NearestAdult,
    SensorType::HoglinSpecific,
];

pub static HOGLIN_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_fight_activity(),
            init_retreat_activity(),
        ]
    },
};
