use std::sync::Arc;

use pumpkin_data::environment_attribute::Activity;

use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::{self, Timed};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::mob::Mob;
use crate::entity::mob::creaking::{ATTACK_INTERVAL, CreakingEntity, SPEED_MULTIPLIER_WHEN_IDLING};

const SWIM_CHANCE: f32 = 0.8;
const MAX_LOOK_DIST: f32 = 8.0;

fn as_creaking(mob: &dyn Mob) -> Option<&CreakingEntity> {
    mob.cast_any().downcast_ref::<CreakingEntity>()
}

fn can_move(mob: &dyn Mob) -> bool {
    as_creaking(mob).is_some_and(CreakingEntity::can_move)
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::with_condition(
                SWIM_CHANCE,
                can_move,
            ))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        10,
        vec![
            Box::new(behavior::start_attacking(
                |tick| as_creaking(tick.mob).is_some_and(CreakingEntity::is_active),
                |tick| {
                    tick.brain
                        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
                        .map(|player| Arc::clone(player) as Arc<dyn EntityBase>)
                },
            )),
            Box::new(behavior::set_entity_look_target_sometimes(
                |_| true,
                MAX_LOOK_DIST,
                30,
                60,
            )),
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
        ],
    )
}

fn init_fight_activity() -> ActivityData {
    ActivityData::with_priority_start_and_conditions(
        Activity::Fight,
        10,
        vec![
            Box::new(behavior::set_walk_target_from_attack_target_if_target_out_of_reach(1.0)),
            Box::new(behavior::melee_attack(
                |tick| can_move(tick.mob),
                ATTACK_INTERVAL,
            )),
            Box::new(behavior::stop_attacking_if_target_invalid(
                |tick, target| !is_attack_target_still_reachable(tick, target),
                |_, _| {},
                true,
            )),
        ],
        vec![(types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent)],
    )
}

/// Vanilla `CreakingAi.isAttackTargetStillReachable`: only players it can still see.
fn is_attack_target_still_reachable(tick: &BrainTick<'_>, target: &Arc<dyn EntityBase>) -> bool {
    let target_id = target.get_entity().entity_id;
    tick.brain
        .get(types::NEAREST_VISIBLE_ATTACKABLE_PLAYERS)
        .is_some_and(|players| players.iter().any(|player| player.entity_id() == target_id))
}

/// Vanilla `CreakingAi.updateActivity`: a frozen creaking drops back to its default activity.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    if can_move(tick.mob) {
        tick.brain
            .set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
    } else {
        tick.brain.use_default_activity();
    }
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestPlayers,
];

pub static CREAKING_PROVIDER: BrainProvider = BrainProvider {
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
