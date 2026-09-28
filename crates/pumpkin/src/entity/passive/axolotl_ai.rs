use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::potion::Effect;
use pumpkin_data::tag;

use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::one_shot::{OneShot, trigger_if};
use crate::entity::ai::brain::behavior::utils::{is_breeding, is_dead_or_dying};
use crate::entity::ai::brain::behavior::{
    self, Behavior, BehaviorControl, GateBehavior, OrderPolicy, RunningPolicy, Timed,
};
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::mob::Mob;
use crate::entity::player::Player;

const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.2;
const SPEED_MULTIPLIER_ON_LAND: f32 = 0.15;
const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f32 = 0.5;
const SPEED_MULTIPLIER_WHEN_CHASING_IN_WATER: f32 = 0.6;
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT_IN_WATER: f32 = 0.6;
const HUNTING_COOLDOWN: i64 = 2400;
const MELEE_ATTACK_COOLDOWN: i32 = 20;
const PLAY_DEAD_DURATION: i32 = 200;
const SUPPORTING_EFFECT_RANGE: f64 = 20.0;
const MAX_REGENERATION_DURATION: i32 = 2400;
const REGENERATION_PER_KILL: i32 = 100;
const INFINITE_DURATION: i32 = -1;

fn in_water(mob: &dyn Mob) -> bool {
    mob.get_entity().is_in_water()
}

fn get_speed_modifier_chasing(mob: &dyn Mob) -> f32 {
    if in_water(mob) {
        SPEED_MULTIPLIER_WHEN_CHASING_IN_WATER
    } else {
        SPEED_MULTIPLIER_ON_LAND
    }
}

fn get_speed_modifier_following_adult(mob: &dyn Mob) -> f32 {
    if in_water(mob) {
        SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT_IN_WATER
    } else {
        SPEED_MULTIPLIER_ON_LAND
    }
}

fn get_speed_modifier(mob: &dyn Mob) -> f32 {
    if in_water(mob) {
        SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER
    } else {
        SPEED_MULTIPLIER_ON_LAND
    }
}

fn init_play_dead_activity() -> ActivityData {
    ActivityData::new(
        Activity::PlayDead,
        vec![
            (0, Box::new(Timed::new(PlayDead))),
            (
                1,
                Box::new(behavior::erase_memory_if(
                    "EraseMemoryIf(isBreeding)",
                    |tick| is_breeding(tick.brain),
                    types::PLAY_DEAD_TICKS.id(),
                )),
            ),
        ],
        vec![(types::PLAY_DEAD_TICKS.id(), MemoryStatus::ValuePresent)],
        vec![types::PLAY_DEAD_TICKS.id()],
    )
}

fn init_fight_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Fight,
        0,
        vec![
            Box::new(behavior::stop_attacking_if_target_invalid(
                |_, _| false,
                on_stop_attacking,
                true,
            )),
            Box::new(
                behavior::set_walk_target_from_attack_target_if_target_out_of_reach_with_speed(
                    get_speed_modifier_chasing,
                ),
            ),
            Box::new(behavior::melee_attack(|_| true, MELEE_ATTACK_COOLDOWN)),
            Box::new(behavior::erase_memory_if(
                "EraseMemoryIf(isBreeding)",
                |tick| is_breeding(tick.brain),
                types::ATTACK_TARGET.id(),
            )),
        ],
        types::ATTACK_TARGET.id(),
    )
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
            Box::new(Timed::new(behavior::MoveToTargetSink::default())),
            Box::new(validate_play_dead()),
            Box::new(Timed::new(behavior::CountDownCooldownTicks::new(
                types::TEMPTATION_COOLDOWN_TICKS,
            ))),
        ],
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "one list, mirroring vanilla AxolotlAi.initIdleActivity"
)]
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
                    &EntityType::AXOLOTL,
                    SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                    2,
                ))),
            ),
            (
                2,
                Box::new(behavior::run_one(vec![
                    (
                        Box::new(Timed::new(behavior::FollowTemptation::new(
                            get_speed_modifier,
                        ))) as Box<dyn BehaviorControl>,
                        1,
                    ),
                    (
                        Box::new(behavior::baby_follow_adult(
                            ADULT_FOLLOW_RANGE.0,
                            ADULT_FOLLOW_RANGE.1,
                            get_speed_modifier_following_adult,
                        )),
                        1,
                    ),
                ])),
            ),
            (
                3,
                Box::new(behavior::start_attacking(
                    |_| true,
                    |tick| {
                        if is_breeding(tick.brain) {
                            None
                        } else {
                            tick.brain.get(types::NEAREST_ATTACKABLE).cloned()
                        }
                    },
                )),
            ),
            (
                3,
                Box::new(behavior::try_find_liquid(
                    6,
                    SPEED_MULTIPLIER_ON_LAND,
                    &tag::Fluid::MINECRAFT_AXOLOTL_TRIES_TO_FIND,
                )),
            ),
            (
                4,
                Box::new(GateBehavior::new(
                    "AxolotlIdleMovement",
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
                            Box::new(behavior::stroll(SPEED_MULTIPLIER_ON_LAND, false)),
                            2,
                        ),
                        (
                            Box::new(behavior::set_walk_target_from_look_target_with_speed(
                                can_set_walk_target_from_look_target,
                                get_speed_modifier,
                                3,
                            )),
                            3,
                        ),
                        (
                            Box::new(trigger_if("TriggerIf(isInWater)", Vec::new(), |tick| {
                                in_water(tick.mob)
                            })),
                            5,
                        ),
                        (
                            Box::new(trigger_if("TriggerIf(onGround)", Vec::new(), |tick| {
                                tick.mob.get_entity().on_ground.load(Ordering::Relaxed)
                            })),
                            5,
                        ),
                    ],
                )),
            ),
        ],
    )
}

fn can_set_walk_target_from_look_target(tick: &BrainTick<'_>) -> bool {
    let Some(look_target) = tick.brain.get(types::LOOK_TARGET) else {
        return false;
    };
    let pos = look_target.current_block_position();
    let water_at = tick
        .world
        .get_block_state_id_if_loaded(&pos)
        .is_some_and(crate::entity::ai::util::goal_utils::is_water_state);
    water_at == in_water(tick.mob)
}

/// Vanilla `AxolotlAi.updateActivity`; playing dead is only ended by `ValidatePlayDead`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    let old_activity = tick.brain.get_active_non_core_activity();
    if old_activity == Some(Activity::PlayDead) {
        return;
    }
    tick.brain.set_active_activity_to_first_valid(&[
        Activity::PlayDead,
        Activity::Fight,
        Activity::Idle,
    ]);
    if old_activity == Some(Activity::Fight)
        && tick.brain.get_active_non_core_activity() != Some(Activity::Fight)
    {
        tick.brain
            .set_with_expiry(types::HAS_HUNTING_COOLDOWN, true, HUNTING_COOLDOWN);
    }
}

/// Vanilla `Axolotl.onStopAttacking`: a kill made for a nearby player heals that player.
fn on_stop_attacking(tick: &mut BrainTick<'_>, target: &Arc<dyn EntityBase>) {
    if !is_dead_or_dying(target.as_ref()) {
        return;
    }
    let Some(living) = target.get_living_entity() else {
        return;
    };
    let Some(player) = tick
        .world
        .get_player_by_id(living.last_attacker_id.load(Ordering::Relaxed))
    else {
        return;
    };
    let range = tick.mob.get_entity().bounding_box.load().expand(
        SUPPORTING_EFFECT_RANGE,
        SUPPORTING_EFFECT_RANGE,
        SUPPORTING_EFFECT_RANGE,
    );
    let in_range = tick
        .world
        .get_players_at_box(&range)
        .iter()
        .any(|nearby| nearby.entity_id() == player.entity_id());
    if in_range {
        apply_supporting_effects(&player);
    }
}

/// Vanilla `Axolotl.applySupportingEffects`.
fn apply_supporting_effects(player: &Player) {
    let regeneration = player.get_effect(&StatusEffect::REGENERATION);
    // Vanilla `endsWithin(2399)`, which an infinite effect never does.
    if regeneration.as_ref().is_none_or(|effect| {
        effect.duration != INFINITE_DURATION && effect.duration < MAX_REGENERATION_DURATION
    }) {
        let previous_duration = regeneration.map_or(0, |effect| effect.duration);
        player.add_effect(Effect {
            effect_type: &StatusEffect::REGENERATION,
            duration: MAX_REGENERATION_DURATION.min(REGENERATION_PER_KILL + previous_duration),
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }
    player.remove_effect(&StatusEffect::MINING_FATIGUE);
}

/// Vanilla `PlayDead`: lies still in water with Regeneration while `PLAY_DEAD_TICKS` lasts.
struct PlayDead;

const PLAY_DEAD_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::PLAY_DEAD_TICKS.id(), MemoryStatus::ValuePresent),
    (types::HURT_BY_ENTITY.id(), MemoryStatus::ValuePresent),
];

impl Behavior for PlayDead {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        PLAY_DEAD_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        PLAY_DEAD_DURATION
    }

    fn max_duration(&self) -> i32 {
        PLAY_DEAD_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        in_water(tick.mob)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        in_water(tick.mob) && tick.brain.has_memory_value(types::PLAY_DEAD_TICKS.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::LOOK_TARGET.id());
        tick.mob.get_mob_entity().living_entity.add_effect(Effect {
            effect_type: &StatusEffect::REGENERATION,
            duration: PLAY_DEAD_DURATION,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }

    fn debug_name(&self) -> &'static str {
        "PlayDead"
    }
}

/// Vanilla `ValidatePlayDead`: counts `PLAY_DEAD_TICKS` down and ends the act at zero.
fn validate_play_dead() -> OneShot {
    OneShot::with_required(
        "ValidatePlayDead",
        vec![(types::PLAY_DEAD_TICKS.id(), MemoryStatus::ValuePresent)],
        vec![types::HURT_BY_ENTITY.id()],
        |tick| {
            let ticks = tick.brain.get(types::PLAY_DEAD_TICKS).copied().unwrap_or(0);
            if ticks <= 0 {
                tick.brain.erase(types::PLAY_DEAD_TICKS.id());
                tick.brain.erase(types::HURT_BY_ENTITY.id());
                tick.brain.use_default_activity();
            } else {
                tick.brain.set(types::PLAY_DEAD_TICKS, ticks - 1);
            }
            true
        },
    )
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::NearestAdult,
    SensorType::HurtBy,
    SensorType::AxolotlAttackables,
    SensorType::FoodTemptations,
];

pub static AXOLOTL_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_fight_activity(),
            init_play_dead_activity(),
        ]
    },
};
