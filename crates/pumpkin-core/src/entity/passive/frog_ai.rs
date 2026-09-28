use std::sync::Arc;

use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::one_shot::trigger_if;
use crate::entity::ai::brain::behavior::utils::{is_alive, is_breeding, look_at_entity};
use crate::entity::ai::brain::behavior::{
    self, Behavior, BehaviorControl, GateBehavior, OrderPolicy, RunningPolicy, Timed,
};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::SensorType;
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::mob::Mob;
use crate::entity::passive::frog::FrogEntity;

const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
const SPEED_MULTIPLIER_ON_LAND: f32 = 1.0;
const SPEED_MULTIPLIER_IN_WATER: f32 = 0.75;
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
const TIME_BETWEEN_LONG_JUMPS: (i32, i32) = (100, 140);
const MAX_LONG_JUMP_HEIGHT: i32 = 2;
const MAX_LONG_JUMP_WIDTH: i32 = 4;
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;
const PREFERRED_JUMP_CHANCE: f32 = 0.5;
const MAX_LOOK_DIST: f32 = 6.0;

/// Vanilla `FrogAi.getTemptations`, shared by frogs and tadpoles.
#[must_use]
pub fn is_temptation(_mob: &dyn Mob, stack: &ItemStack) -> bool {
    stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
}

/// Vanilla `Frog.canEat`: frog food, and only the smallest slimes and magma cubes.
#[must_use]
pub fn can_eat(entity: &dyn EntityBase) -> bool {
    let entity = entity.get_entity();
    let cube =
        entity.entity_type == &EntityType::SLIME || entity.entity_type == &EntityType::MAGMA_CUBE;
    if cube && entity.data.load(std::sync::atomic::Ordering::Relaxed) != 1 {
        return false;
    }
    entity
        .entity_type
        .has_tag(&tag::EntityType::MINECRAFT_FROG_FOOD)
}

/// Vanilla `FrogAi.initMemories`.
pub fn init_memories(brain: &mut Brain, mob: &dyn Mob) {
    let cooldown = mob
        .get_random()
        .random_range(TIME_BETWEEN_LONG_JUMPS.0..=TIME_BETWEEN_LONG_JUMPS.1);
    brain.set(types::LONG_JUMP_COOLDOWN_TICKS, cooldown);
}

fn as_frog(mob: &dyn Mob) -> Option<&FrogEntity> {
    mob.cast_any().downcast_ref::<FrogEntity>()
}

fn look_at_player_sometimes() -> Box<dyn BehaviorControl> {
    Box::new(behavior::set_entity_look_target_sometimes(
        |entity| entity.get_entity().entity_type == &EntityType::PLAYER,
        MAX_LOOK_DIST,
        30,
        60,
    ))
}

fn follow_temptation() -> Box<dyn BehaviorControl> {
    Box::new(Timed::new(behavior::FollowTemptation::new(|_| {
        SPEED_MULTIPLIER_WHEN_TEMPTED
    })))
}

fn start_attacking() -> Box<dyn BehaviorControl> {
    Box::new(behavior::start_attacking(
        |tick| !is_breeding(tick.brain),
        |tick| tick.brain.get(types::NEAREST_ATTACKABLE).cloned(),
    ))
}

fn trigger_if_on_ground() -> Box<dyn BehaviorControl> {
    Box::new(trigger_if("TriggerIf(onGround)", Vec::new(), |tick| {
        tick.mob
            .get_entity()
            .on_ground
            .load(std::sync::atomic::Ordering::Relaxed)
    }))
}

fn not_mid_jump_and(
    memory: MemoryModuleId,
    status: MemoryStatus,
) -> Vec<(MemoryModuleId, MemoryStatus)> {
    vec![
        (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValueAbsent),
        (memory, status),
    ]
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
                types::LONG_JUMP_COOLDOWN_TICKS,
            ))),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Idle,
        vec![
            (0, look_at_player_sometimes()),
            (
                0,
                Box::new(Timed::new(behavior::AnimalMakeLove::new(
                    &EntityType::FROG,
                    1.0,
                    2,
                ))),
            ),
            (1, follow_temptation()),
            (2, start_attacking()),
            (
                3,
                Box::new(behavior::try_find_land(6, SPEED_MULTIPLIER_WHEN_IDLING)),
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
                                SPEED_MULTIPLIER_ON_LAND,
                                3,
                            )),
                            1,
                        ),
                        (Box::new(Timed::new(Croak::default())), 3),
                        (trigger_if_on_ground(), 2),
                    ],
                )),
            ),
        ],
        not_mid_jump_and(types::IS_IN_WATER.id(), MemoryStatus::ValueAbsent),
    )
}

fn init_swim_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::Swim,
        vec![
            (0, look_at_player_sometimes()),
            (1, follow_temptation()),
            (2, start_attacking()),
            (3, Box::new(behavior::try_find_land(8, 1.5))),
            (
                5,
                Box::new(GateBehavior::new(
                    "FrogSwimMovement",
                    vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
                    Vec::new(),
                    OrderPolicy::Ordered,
                    RunningPolicy::TryAll,
                    vec![
                        (Box::new(behavior::swim(SPEED_MULTIPLIER_IN_WATER)), 1),
                        (
                            Box::new(behavior::stroll(SPEED_MULTIPLIER_ON_LAND, true)),
                            1,
                        ),
                        (
                            Box::new(behavior::set_walk_target_from_look_target(
                                |_| true,
                                SPEED_MULTIPLIER_ON_LAND,
                                3,
                            )),
                            1,
                        ),
                        (
                            Box::new(trigger_if("TriggerIf(isInWater)", Vec::new(), |tick| {
                                tick.mob.get_entity().is_in_water()
                            })),
                            5,
                        ),
                    ],
                )),
            ),
        ],
        not_mid_jump_and(types::IS_IN_WATER.id(), MemoryStatus::ValuePresent),
    )
}

fn init_lay_spawn_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::LaySpawn,
        vec![
            (0, look_at_player_sometimes()),
            (1, start_attacking()),
            (
                2,
                Box::new(behavior::try_find_land_near_liquid(
                    8,
                    SPEED_MULTIPLIER_ON_LAND,
                    &tag::Fluid::MINECRAFT_FROG_TRIES_TO_FIND_LAND_NEAR,
                )),
            ),
            (
                3,
                Box::new(behavior::try_lay_spawn_on_fluid_near_land(
                    &Block::FROGSPAWN,
                )),
            ),
            (
                4,
                Box::new(behavior::run_one(vec![
                    (
                        Box::new(behavior::stroll(SPEED_MULTIPLIER_ON_LAND, true)),
                        2,
                    ),
                    (
                        Box::new(behavior::set_walk_target_from_look_target(
                            |_| true,
                            SPEED_MULTIPLIER_ON_LAND,
                            3,
                        )),
                        1,
                    ),
                    (Box::new(Timed::new(Croak::default())), 2),
                    (trigger_if_on_ground(), 1),
                ])),
            ),
        ],
        not_mid_jump_and(types::IS_PREGNANT.id(), MemoryStatus::ValuePresent),
    )
}

fn init_jump_activity() -> ActivityData {
    ActivityData::with_pairs_and_conditions(
        Activity::LongJump,
        vec![
            (
                0,
                Box::new(Timed::new(behavior::LongJumpMidJump::new(
                    TIME_BETWEEN_LONG_JUMPS,
                    Sound::EntityFrogStep,
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
                        Sound::EntityFrogLongJump,
                        is_acceptable_landing_spot,
                    )
                    .preferring(
                        &tag::Block::MINECRAFT_FROG_PREFER_JUMP_TO,
                        PREFERRED_JUMP_CHANCE,
                    ),
                )),
            ),
        ],
        vec![
            (types::TEMPTING_PLAYER.id(), MemoryStatus::ValueAbsent),
            (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
            (
                types::LONG_JUMP_COOLDOWN_TICKS.id(),
                MemoryStatus::ValueAbsent,
            ),
            (types::IS_IN_WATER.id(), MemoryStatus::ValueAbsent),
        ],
    )
}

fn init_tongue_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Tongue,
        0,
        vec![
            Box::new(behavior::stop_attacking_if_target_invalid(
                |_, _| false,
                |_, _| {},
                true,
            )),
            Box::new(Timed::new(ShootTongue::default())),
        ],
        types::ATTACK_TARGET.id(),
    )
}

fn has_fluid(state_id: Option<BlockStateId>) -> bool {
    state_id.is_some_and(|state_id| {
        state_id.to_state().is_waterlogged()
            || pumpkin_data::fluid::Fluid::from_state_id(state_id).is_some()
    })
}

/// Vanilla `FrogAi.isAcceptableLandingSpot`.
fn is_acceptable_landing_spot(mob: &dyn Mob, target_pos: &BlockPos) -> bool {
    let world = mob.get_entity().world.load_full();
    let below = target_pos.down();
    if has_fluid(world.get_block_state_id_if_loaded(target_pos))
        || has_fluid(world.get_block_state_id_if_loaded(&below))
        || has_fluid(world.get_block_state_id_if_loaded(&target_pos.up()))
    {
        return false;
    }
    let (block, state) = world.get_block_and_state(target_pos);
    let prefer = &tag::Block::MINECRAFT_FROG_PREFER_JUMP_TO;
    if block.has_tag(prefer) || world.get_block(&below).has_tag(prefer) {
        return true;
    }
    let is_air = state.is_air();
    let mut context =
        PathfindingContext::new(mob.get_entity().block_pos.load().0, Arc::clone(&world));
    let path_type = context.get_land_node_type(target_pos.0);
    let path_type_below = context.get_land_node_type(below.0);
    if path_type != PathType::Trapdoor && (!is_air || path_type_below != PathType::Trapdoor) {
        behavior::default_acceptable_landing_spot(mob, target_pos)
    } else {
        true
    }
}

/// Vanilla `FrogAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain.set_active_activity_to_first_valid(&[
        Activity::Tongue,
        Activity::LaySpawn,
        Activity::LongJump,
        Activity::Swim,
        Activity::Idle,
    ]);
}

const CROAK_TICKS: i32 = 60;
const CROAK_TIME_OUT_DURATION: i32 = 100;
const CROAK_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] =
    &[(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)];

/// Vanilla `Croak`: stands still in the croaking pose for three seconds.
#[derive(Default)]
struct Croak {
    croak_counter: i32,
}

impl Behavior for Croak {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        CROAK_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        CROAK_TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        CROAK_TIME_OUT_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.mob.get_entity().pose.load() == EntityPose::Standing
    }

    fn can_still_use(&mut self, _tick: &BrainTick<'_>) -> bool {
        self.croak_counter < CROAK_TICKS
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        let in_liquid = entity.is_in_water()
            || entity
                .touching_lava
                .load(std::sync::atomic::Ordering::Relaxed);
        if !in_liquid {
            entity.set_pose(EntityPose::Croaking);
            self.croak_counter = 0;
        }
    }

    fn tick(&mut self, _tick: &mut BrainTick<'_>) {
        self.croak_counter += 1;
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.mob.get_entity().set_pose(EntityPose::Standing);
    }

    fn debug_name(&self) -> &'static str {
        "Croak"
    }
}

const TONGUE_TIME_OUT_DURATION: i32 = 100;
const CATCH_ANIMATION_DURATION: i32 = 6;
const TONGUE_ANIMATION_DURATION: i32 = 10;
const EATING_DISTANCE: f64 = 1.75;
const EATING_MOVEMENT_FACTOR: f64 = 0.75;
const UNREACHABLE_TONGUE_TARGETS_COOLDOWN_DURATION: i64 = 100;
const MAX_UNREACHABLE_TONGUE_TARGETS_IN_MEMORY: usize = 5;
const TONGUE_WALK_SPEED: f32 = 2.0;
const RECALCULATE_PATH_INTERVAL: i32 = 10;
const SHOOT_TONGUE_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
];

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum TongueState {
    MoveToTarget,
    CatchAnimation,
    EatAnimation,
    #[default]
    Done,
}

/// Vanilla `ShootTongue`: walks up to a small slime or magma cube and eats it.
#[derive(Default)]
struct ShootTongue {
    eat_animation_timer: i32,
    calculate_path_counter: i32,
    state: TongueState,
}

impl ShootTongue {
    fn can_pathfind_to_target(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        let mob_entity = mob.get_mob_entity();
        mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .create_path(&mob_entity.living_entity, target.get_entity().pos.load(), 0)
            .is_some_and(|path| f64::from(path.get_dist_to_target()) < EATING_DISTANCE)
    }

    fn add_unreachable_target_to_memory(brain: &mut Brain, target: &dyn EntityBase) {
        let uuid = target.get_entity().entity_uuid;
        let mut targets = brain
            .get(types::UNREACHABLE_TONGUE_TARGETS)
            .cloned()
            .unwrap_or_default();
        let should_add = !targets.contains(&uuid);
        if targets.len() == MAX_UNREACHABLE_TONGUE_TARGETS_IN_MEMORY && should_add {
            targets.remove(0);
        }
        if should_add {
            targets.push(uuid);
        }
        brain.set_with_expiry(
            types::UNREACHABLE_TONGUE_TARGETS,
            targets,
            UNREACHABLE_TONGUE_TARGETS_COOLDOWN_DURATION,
        );
    }

    fn eat_entity(tick: &BrainTick<'_>) {
        let entity = tick.mob.get_entity();
        tick.world.play_sound_fine(
            Sound::EntityFrogEat,
            SoundCategory::Neutral,
            &entity.pos.load(),
            2.0,
            1.0,
        );
        let Some(target) = as_frog(tick.mob).and_then(|frog| frog.get_tongue_target(tick.world))
        else {
            return;
        };
        if is_alive(target.as_ref()) {
            tick.mob.do_hurt_target(target.as_ref());
            if !is_alive(target.as_ref()) {
                target.get_entity().remove();
            }
        }
    }
}

impl Behavior for ShootTongue {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SHOOT_TONGUE_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        TONGUE_TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        TONGUE_TIME_OUT_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let Some(target) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return false;
        };
        let can_pathfind = Self::can_pathfind_to_target(tick.mob, target.as_ref());
        if !can_pathfind {
            tick.brain.erase(types::ATTACK_TARGET.id());
            Self::add_unreachable_target_to_memory(tick.brain, target.as_ref());
        }
        can_pathfind
            && tick.mob.get_entity().pose.load() != EntityPose::Croaking
            && can_eat(target.as_ref())
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::ATTACK_TARGET.id())
            && self.state != TongueState::Done
            && !tick.brain.has_memory_value(types::IS_PANICKING.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return;
        };
        look_at_entity(tick.brain, Arc::clone(&target));
        if let Some(frog) = as_frog(tick.mob) {
            frog.set_tongue_target(target.as_ref());
        }
        tick.brain.set(
            types::WALK_TARGET,
            WalkTarget::from_vec(target.get_entity().pos.load(), TONGUE_WALK_SPEED, 0),
        );
        self.calculate_path_counter = RECALCULATE_PATH_INTERVAL;
        self.state = TongueState::MoveToTarget;
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return;
        };
        if let Some(frog) = as_frog(tick.mob) {
            frog.set_tongue_target(target.as_ref());
        }
        match self.state {
            TongueState::MoveToTarget => {
                let body = tick.mob.get_entity();
                let body_pos = body.pos.load();
                let target_entity = target.get_entity();
                let target_pos = target_entity.pos.load();
                if target_pos.squared_distance_to_vec(&body_pos).sqrt() < EATING_DISTANCE {
                    tick.world.play_sound_fine(
                        Sound::EntityFrogTongue,
                        SoundCategory::Neutral,
                        &body_pos,
                        2.0,
                        1.0,
                    );
                    body.set_pose(EntityPose::UsingTongue);
                    target_entity
                        .set_velocity((body_pos - target_pos).normalize() * EATING_MOVEMENT_FACTOR);
                    self.eat_animation_timer = 0;
                    self.state = TongueState::CatchAnimation;
                } else if self.calculate_path_counter <= 0 {
                    tick.brain.set(
                        types::WALK_TARGET,
                        WalkTarget::from_vec(target_pos, TONGUE_WALK_SPEED, 0),
                    );
                    self.calculate_path_counter = RECALCULATE_PATH_INTERVAL;
                } else {
                    self.calculate_path_counter -= 1;
                }
            }
            TongueState::CatchAnimation => {
                let timer = self.eat_animation_timer;
                self.eat_animation_timer += 1;
                if timer >= CATCH_ANIMATION_DURATION {
                    self.state = TongueState::EatAnimation;
                    Self::eat_entity(tick);
                }
            }
            TongueState::EatAnimation => {
                if self.eat_animation_timer >= TONGUE_ANIMATION_DURATION {
                    self.state = TongueState::Done;
                } else {
                    self.eat_animation_timer += 1;
                }
            }
            TongueState::Done => {}
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::ATTACK_TARGET.id());
        if let Some(frog) = as_frog(tick.mob) {
            frog.erase_tongue_target();
        }
        tick.mob.get_entity().set_pose(EntityPose::Standing);
    }

    fn debug_name(&self) -> &'static str {
        "ShootTongue"
    }
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::HurtBy,
    SensorType::FrogAttackables,
    SensorType::FrogTemptations,
    SensorType::IsInWater,
];

pub static FROG_PROVIDER: BrainProvider = BrainProvider {
    memory_types: MEMORY_TYPES,
    sensor_types: SENSOR_TYPES,
    activities: |_| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_swim_activity(),
            init_lay_spawn_activity(),
            init_tongue_activity(),
            init_jump_activity(),
        ]
    },
};
