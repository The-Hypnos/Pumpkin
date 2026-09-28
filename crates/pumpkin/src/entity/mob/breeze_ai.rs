use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::seq::SliceRandom;

use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::long_jump::calculate_jump_vector_for_angle;
use crate::entity::ai::brain::behavior::random_look_around::direction_from_rotation;
use crate::entity::ai::brain::behavior::{self, Behavior, Timed};
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus, types};
use crate::entity::ai::brain::sensing::{SensorType, is_entity_attackable};
use crate::entity::ai::brain::{BrainProvider, BrainTick};
use crate::entity::ai::goal::swim::SwimGoal;
use crate::entity::ai::util::default_random_pos;
use crate::entity::ai::util::goal_utils::fluid_has_tag;
use crate::entity::mob::Mob;
use crate::entity::projectile::ThrownItemEntity;
use crate::entity::projectile::wind_charge::{WIND_CHARGE_GRAVITY, WindChargeEntity};
use crate::entity::{Entity, EntityBase};
use crate::world::World;
use crate::world::natural_spawner::is_block_dangerous;

pub const SPEED_MULTIPLIER_WHEN_SLIDING: f32 = 0.6;
pub const JUMP_CIRCLE_INNER_RADIUS: f64 = 4.0;
pub const JUMP_CIRCLE_MIDDLE_RADIUS: f64 = 8.0;
const TICKS_TO_REMEMBER_SEEN_TARGET: i32 = 100;
const SWIM_CHANCE: f32 = 0.8;
const INNER_CIRCLE_VERTICAL_RANGE: f64 = 10.0;

fn pose(mob: &dyn Mob) -> EntityPose {
    mob.get_entity().pose.load()
}

fn set_pose(mob: &dyn Mob, pose: EntityPose) {
    mob.get_entity().set_pose(pose);
}

fn play_sound(tick: &BrainTick<'_>, sound: Sound, volume: f32) {
    tick.world.play_sound_fine(
        sound,
        SoundCategory::Hostile,
        &tick.mob.get_entity().pos.load(),
        volume,
        1.0,
    );
}

fn on_ground(mob: &dyn Mob) -> bool {
    mob.get_entity().on_ground.load(Ordering::Relaxed)
}

fn follow_range(mob: &dyn Mob) -> f64 {
    mob.get_mob_entity()
        .living_entity
        .get_attribute_value(&Attributes::FOLLOW_RANGE)
}

/// Vanilla `Breeze.getFiringYPosition`.
fn firing_y_position(mob: &dyn Mob) -> f64 {
    let entity = mob.get_entity();
    entity.pos.load().y + f64::from(entity.height()) / 2.0 + f64::from(0.3f32)
}

/// Vanilla `Breeze.withinInnerCircleRange`.
fn within_inner_circle_range(mob: &dyn Mob, target: Vector3<f64>) -> bool {
    let center = mob.get_entity().block_pos.load().to_centered_f64();
    let dx = target.x - center.x;
    let dz = target.z - center.z;
    dx.mul_add(dx, dz * dz) < JUMP_CIRCLE_INNER_RADIUS * JUMP_CIRCLE_INNER_RADIUS
        && (target.y - center.y).abs() < INNER_CIRCLE_VERTICAL_RANGE
}

/// Vanilla `Entity.lookAt(EYES, target)`.
fn look_at_eyes(mob: &dyn Mob, target: Vector3<f64>) {
    let entity = mob.get_entity();
    let eye = entity.get_eye_pos();
    let dx = target.x - eye.x;
    let dy = target.y - eye.y;
    let dz = target.z - eye.z;
    let pitch = -(dy.atan2(dx.hypot(dz)).to_degrees() as f32);
    let yaw = (dz.atan2(dx).to_degrees() as f32) - 90.0;
    entity.set_rotation(yaw, pitch);
    entity.head_yaw.store(yaw);
}

fn has_collision(world: &World, pos: &BlockPos) -> bool {
    world
        .get_block_state(pos)
        .get_block_collision_shapes()
        .next()
        .is_some()
}

/// Vanilla `BreezeUtil.hasLineOfSight`: an unobstructed straight line from the breeze's feet.
fn has_line_of_sight(tick: &BrainTick<'_>, target: Vector3<f64>) -> bool {
    let from = tick.mob.get_entity().pos.load();
    if (target - from).length() > follow_range(tick.mob).max(50.0) {
        return false;
    }
    tick.world
        .raycast(from, target, |pos, world| has_collision(world, pos))
        .is_none()
}

/// Vanilla `BreezeUtil.randomPointBehindTarget`.
fn random_point_behind_target(enemy: &dyn EntityBase, mob: &dyn Mob) -> Vector3<f64> {
    let mut rng = mob.get_random();
    // Box-Muller stands in for `RandomSource.nextGaussian`.
    let u1: f64 = rng.random::<f64>().max(f64::MIN_POSITIVE);
    let u2: f64 = rng.random();
    let gaussian = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
    let enemy_entity = enemy.get_entity();
    let view_angle = enemy_entity.head_yaw.load() + 180.0 + (gaussian as f32) * 90.0 / 2.0;
    let r = rng.random::<f32>().mul_add(
        JUMP_CIRCLE_MIDDLE_RADIUS as f32 - JUMP_CIRCLE_INNER_RADIUS as f32,
        JUMP_CIRCLE_INNER_RADIUS as f32,
    );
    enemy_entity.pos.load() + direction_from_rotation(0.0, view_angle) * f64::from(r)
}

fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(Timed::new(behavior::Swim::new(SWIM_CHANCE))),
            Box::new(Timed::new(behavior::LookAtTargetSink::new(45, 90))),
        ],
    )
}

fn init_idle_activity() -> ActivityData {
    ActivityData::with_pairs(
        Activity::Idle,
        vec![
            (
                0,
                Box::new(behavior::start_attacking(
                    |_| true,
                    |tick| tick.brain.get(types::NEAREST_ATTACKABLE).cloned(),
                )),
            ),
            (
                1,
                Box::new(behavior::start_attacking(
                    |_| true,
                    |tick| {
                        // Vanilla `Breeze.getHurtBy`.
                        tick.brain
                            .get(types::HURT_BY)
                            .and_then(|hurt_by| hurt_by.attacker.clone())
                            .filter(|attacker| attacker.get_living_entity().is_some())
                    },
                )),
            ),
            (
                2,
                Box::new(Timed::new(SlideToTargetSink(
                    behavior::MoveToTargetSink::new(20, 40),
                ))),
            ),
            (
                3,
                Box::new(behavior::run_one(vec![
                    (Box::new(behavior::DoNothing::new(20, 100)), 1),
                    (
                        Box::new(behavior::stroll(SPEED_MULTIPLIER_WHEN_SLIDING, true)),
                        2,
                    ),
                ])),
            ),
        ],
    )
}

fn init_fight_activity() -> ActivityData {
    // Vanilla `Sensor.wasEntityAttackableLastNTicks(body, 100).negate()`.
    let positives_left = AtomicI32::new(0);
    ActivityData::with_pairs_and_conditions(
        Activity::Fight,
        vec![
            (
                0,
                Box::new(behavior::stop_attacking_if_target_invalid(
                    move |tick, target| {
                        if is_entity_attackable(&tick.visibility(), target.as_ref()) {
                            positives_left.store(TICKS_TO_REMEMBER_SEEN_TARGET, Ordering::Relaxed);
                            false
                        } else {
                            positives_left.fetch_sub(1, Ordering::Relaxed) <= 0
                        }
                    },
                    |_, _| {},
                    true,
                )),
            ),
            (1, Box::new(Timed::new(Shoot))),
            (2, Box::new(Timed::new(LongJump))),
            (3, Box::new(Timed::new(ShootWhenStuck))),
            (4, Box::new(Timed::new(Slide))),
        ],
        vec![
            (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
            (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        ],
    )
}

/// Vanilla `BreezeAi.updateActivity`.
pub fn update_activity(tick: &mut BrainTick<'_>) {
    tick.brain
        .set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}

/// Vanilla `BreezeAi.SlideToTargetSink`: slides while walking, then readies a shot.
struct SlideToTargetSink(behavior::MoveToTargetSink);

impl Behavior for SlideToTargetSink {
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
        self.0.start(tick);
        play_sound(tick, Sound::EntityBreezeSlide, 1.0);
        set_pose(tick.mob, EntityPose::Sliding);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        self.0.tick(tick);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        self.0.stop(tick);
        set_pose(tick.mob, EntityPose::Standing);
        if tick.brain.has_memory_value(types::ATTACK_TARGET.id()) {
            tick.brain.set_with_expiry(types::BREEZE_SHOOT, (), 60);
        }
    }

    fn debug_name(&self) -> &'static str {
        "SlideToTargetSink"
    }
}

const SHOOT_ATTACK_RANGE_SQUARED: f64 = 256.0;
const SHOOT_INITIAL_DELAY_TICKS: i64 = 15;
const SHOOT_RECOVER_DELAY_TICKS: i64 = 4;
const SHOOT_COOLDOWN_TICKS: i64 = 10;
const PROJECTILE_MOVEMENT_SCALE: f64 = 0.7;
const UNCERTAINTY_BASE: i32 = 5;
const UNCERTAINTY_MULTIPLIER: i32 = 4;
const SHOOT_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_SHOOT_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT_CHARGING.id(), MemoryStatus::ValueAbsent),
    (
        types::BREEZE_SHOOT_RECOVERING.id(),
        MemoryStatus::ValueAbsent,
    ),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla breeze `Shoot`: inhales, then fires a wind charge at the target.
struct Shoot;

impl Behavior for Shoot {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SHOOT_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        (SHOOT_INITIAL_DELAY_TICKS + 1 + SHOOT_RECOVER_DELAY_TICKS) as i32
    }

    fn max_duration(&self) -> i32 {
        self.min_duration()
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if pose(tick.mob) != EntityPose::Standing {
            return false;
        }
        let Some(target) = tick.brain.get(types::ATTACK_TARGET) else {
            return false;
        };
        let within_range = tick
            .mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load())
            < SHOOT_ATTACK_RANGE_SQUARED;
        if !within_range {
            tick.brain.erase(types::BREEZE_SHOOT.id());
        }
        within_range
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::ATTACK_TARGET.id())
            && tick.brain.has_memory_value(types::BREEZE_SHOOT.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if tick.brain.has_memory_value(types::ATTACK_TARGET.id()) {
            set_pose(tick.mob, EntityPose::Shooting);
        }
        tick.brain
            .set_with_expiry(types::BREEZE_SHOOT_CHARGING, (), SHOOT_INITIAL_DELAY_TICKS);
        play_sound(tick, Sound::EntityBreezeInhale, 1.0);
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return;
        };
        let target_entity = target.get_entity();
        look_at_eyes(tick.mob, target_entity.pos.load());
        if tick
            .brain
            .has_memory_value(types::BREEZE_SHOOT_CHARGING.id())
            || tick
                .brain
                .has_memory_value(types::BREEZE_SHOOT_RECOVERING.id())
        {
            return;
        }
        tick.brain.set_with_expiry(
            types::BREEZE_SHOOT_RECOVERING,
            (),
            SHOOT_RECOVER_DELAY_TICKS,
        );
        let body = tick.mob.get_entity();
        let body_pos = body.pos.load();
        let target_pos = target_entity.pos.load();
        let height_factor = if target_entity.has_vehicle() {
            0.8
        } else {
            0.3
        };
        let firing_y = firing_y_position(tick.mob);
        let difficulty = tick.world.level_info.load().difficulty as i32;
        let uncertainty = UNCERTAINTY_BASE - difficulty * UNCERTAINTY_MULTIPLIER;
        let thrown = ThrownItemEntity {
            entity: Entity::new(
                Arc::clone(tick.world),
                Vector3::new(body_pos.x, firing_y, body_pos.z),
                &EntityType::BREEZE_WIND_CHARGE,
            ),
            owner_id: Some(body.entity_id),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: WIND_CHARGE_GRAVITY,
        };
        thrown.set_velocity(
            target_pos.x - body_pos.x,
            f64::from(target_entity.height()).mul_add(height_factor, target_pos.y) - firing_y,
            target_pos.z - body_pos.z,
            PROJECTILE_MOVEMENT_SCALE,
            f64::from(uncertainty),
        );
        tick.world
            .spawn_entity(Arc::new(WindChargeEntity::new_breeze(thrown)));
        play_sound(tick, Sound::EntityBreezeShoot, 1.5);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if pose(tick.mob) == EntityPose::Shooting {
            set_pose(tick.mob, EntityPose::Standing);
        }
        tick.brain
            .set_with_expiry(types::BREEZE_SHOOT_COOLDOWN, (), SHOOT_COOLDOWN_TICKS);
        tick.brain.erase(types::BREEZE_SHOOT.id());
    }

    fn debug_name(&self) -> &'static str {
        "Shoot"
    }
}

const REQUIRED_AIR_BLOCKS_ABOVE: i32 = 4;
const JUMP_COOLDOWN_TICKS: i64 = 10;
const JUMP_COOLDOWN_WHEN_HURT_TICKS: i64 = 2;
const INHALING_DURATION_TICKS: i64 = 10;
const LONG_JUMP_TIME_OUT: i32 = 200;
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 0.058_333_334;
const ALLOWED_JUMP_ANGLES: [i32; 5] = [40, 55, 60, 75, 80];
const MIN_JUMP_DISTANCE: f64 = 4.0;
const SNAP_TO_SURFACE_RANGE: f64 = 10.0;
const LONG_JUMP_SHOOT_WINDOW: i64 = 100;
const LONG_JUMP_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_JUMP_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_INHALING.id(), MemoryStatus::Registered),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::Registered),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_LEAVING_WATER.id(), MemoryStatus::Registered),
];

/// Vanilla breeze `LongJump`: inhales, then leaps to a spot behind the target.
struct LongJump;

impl LongJump {
    fn can_run(tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        if !on_ground(tick.mob) && !entity.is_in_water() {
            return false;
        }
        if SwimGoal::is_in_fluid(tick.mob) {
            return false;
        }
        if tick.brain.has_memory_value(types::BREEZE_JUMP_TARGET.id()) {
            return true;
        }
        let Some(target) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return false;
        };
        let body_pos = entity.pos.load();
        let distance_squared = target
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&body_pos);
        let follow_range = follow_range(tick.mob);
        if distance_squared >= follow_range * follow_range {
            tick.brain.erase(types::ATTACK_TARGET.id());
            return false;
        }
        if distance_squared.sqrt() - MIN_JUMP_DISTANCE <= 0.0 {
            return false;
        }
        if !Self::can_jump_from_current_position(tick) {
            return false;
        }
        let behind = random_point_behind_target(target.as_ref(), tick.mob);
        let Some(target_block) = Self::snap_to_surface(tick, behind) else {
            return false;
        };
        let below = tick.world.get_block_state(&target_block.down());
        if is_block_dangerous(tick.mob.get_entity().entity_type, below) {
            return false;
        }
        if !has_line_of_sight(tick, target_block.to_centered_f64())
            && !has_line_of_sight(
                tick,
                target_block
                    .up_height(REQUIRED_AIR_BLOCKS_ABOVE)
                    .to_centered_f64(),
            )
        {
            return false;
        }
        tick.brain.set(types::BREEZE_JUMP_TARGET, target_block);
        true
    }

    /// Vanilla `LongJump.snapToSurface`: the block above the first surface ten blocks down, or up.
    fn snap_to_surface(tick: &BrainTick<'_>, target: Vector3<f64>) -> Option<BlockPos> {
        [-SNAP_TO_SURFACE_RANGE, SNAP_TO_SURFACE_RANGE]
            .into_iter()
            .find_map(|offset| {
                let end = Vector3::new(target.x, target.y + offset, target.z);
                tick.world
                    .raycast(target, end, |pos, world| has_collision(world, pos))
            })
            .map(|(hit, face)| {
                let hit_y = if face == BlockDirection::Up {
                    f64::from(hit.0.y + 1)
                } else {
                    f64::from(hit.0.y)
                };
                BlockPos::floored(target.x, hit_y, target.z).up()
            })
    }

    fn can_jump_from_current_position(tick: &BrainTick<'_>) -> bool {
        let current = tick.mob.get_entity().block_pos.load();
        if tick.world.get_block(&current) == &Block::HONEY_BLOCK {
            return false;
        }
        (1..=REQUIRED_AIR_BLOCKS_ABOVE).all(|i| {
            let pos = current.up_height(i);
            tick.world.get_block_state(&pos).is_air()
                || fluid_has_tag(
                    tick.world.get_block_state_id(&pos),
                    &tag::Fluid::MINECRAFT_WATER,
                )
        })
    }

    fn calculate_optimal_jump_vector(
        tick: &BrainTick<'_>,
        target: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let mut angles = ALLOWED_JUMP_ANGLES;
        angles.shuffle(&mut tick.mob.get_random());
        let living = &tick.mob.get_mob_entity().living_entity;
        let max_jump_velocity = MAX_JUMP_VELOCITY_MULTIPLIER * follow_range(tick.mob) as f32;
        angles.into_iter().find_map(|angle| {
            let velocity = calculate_jump_vector_for_angle(
                tick.mob,
                target,
                max_jump_velocity,
                angle,
                false,
                1.0,
            )?;
            if living.has_effect(&StatusEffect::JUMP_BOOST) {
                let boost = velocity.normalize().y * living.get_jump_boost_power();
                return Some(velocity + Vector3::new(0.0, boost, 0.0));
            }
            Some(velocity)
        })
    }

    fn is_finished_inhaling(tick: &BrainTick<'_>) -> bool {
        !tick
            .brain
            .has_memory_value(types::BREEZE_JUMP_INHALING.id())
            && pose(tick.mob) == EntityPose::Inhaling
    }

    fn is_finished_jumping(tick: &BrainTick<'_>) -> bool {
        let landed_in_water = tick.mob.get_entity().is_in_water()
            && !tick
                .brain
                .has_memory_value(types::BREEZE_LEAVING_WATER.id());
        pose(tick.mob) == EntityPose::LongJumping && (on_ground(tick.mob) || landed_in_water)
    }
}

impl Behavior for LongJump {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        LONG_JUMP_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        LONG_JUMP_TIME_OUT
    }

    fn max_duration(&self) -> i32 {
        LONG_JUMP_TIME_OUT
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        Self::can_run(tick)
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        pose(tick.mob) != EntityPose::Standing
            && !tick
                .brain
                .has_memory_value(types::BREEZE_JUMP_COOLDOWN.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if !tick
            .brain
            .has_memory_value(types::BREEZE_JUMP_INHALING.id())
        {
            tick.brain
                .set_with_expiry(types::BREEZE_JUMP_INHALING, (), INHALING_DURATION_TICKS);
        }
        set_pose(tick.mob, EntityPose::Inhaling);
        play_sound(tick, Sound::EntityBreezeCharge, 1.0);
        if let Some(target) = tick.brain.get(types::BREEZE_JUMP_TARGET).copied() {
            look_at_eyes(tick.mob, target.to_centered_f64());
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let in_water = tick.mob.get_entity().is_in_water();
        if !in_water
            && tick
                .brain
                .has_memory_value(types::BREEZE_LEAVING_WATER.id())
        {
            tick.brain.erase(types::BREEZE_LEAVING_WATER.id());
        }
        if Self::is_finished_inhaling(tick) {
            let velocity = tick
                .brain
                .get(types::BREEZE_JUMP_TARGET)
                .copied()
                .and_then(|target| {
                    let bottom_center = Vector3::new(
                        f64::from(target.0.x) + 0.5,
                        f64::from(target.0.y),
                        f64::from(target.0.z) + 0.5,
                    );
                    Self::calculate_optimal_jump_vector(tick, bottom_center)
                });
            let Some(velocity) = velocity else {
                set_pose(tick.mob, EntityPose::Standing);
                return;
            };
            if in_water {
                tick.brain.set(types::BREEZE_LEAVING_WATER, ());
            }
            play_sound(tick, Sound::EntityBreezeJump, 1.0);
            set_pose(tick.mob, EntityPose::LongJumping);
            let living = &tick.mob.get_mob_entity().living_entity;
            let entity = &living.entity;
            entity.set_rotation(entity.body_yaw.load(), entity.pitch.load());
            living.discard_friction.store(true, Ordering::Relaxed);
            entity.set_velocity(velocity);
        } else if Self::is_finished_jumping(tick) {
            play_sound(tick, Sound::EntityBreezeLand, 1.0);
            set_pose(tick.mob, EntityPose::Standing);
            tick.mob
                .get_mob_entity()
                .living_entity
                .discard_friction
                .store(false, Ordering::Relaxed);
            let cooldown = if tick.brain.has_memory_value(types::HURT_BY.id()) {
                JUMP_COOLDOWN_WHEN_HURT_TICKS
            } else {
                JUMP_COOLDOWN_TICKS
            };
            tick.brain
                .set_with_expiry(types::BREEZE_JUMP_COOLDOWN, (), cooldown);
            tick.brain
                .set_with_expiry(types::BREEZE_SHOOT, (), LONG_JUMP_SHOOT_WINDOW);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if matches!(
            pose(tick.mob),
            EntityPose::LongJumping | EntityPose::Inhaling
        ) {
            set_pose(tick.mob, EntityPose::Standing);
        }
        tick.brain.erase(types::BREEZE_JUMP_TARGET.id());
        tick.brain.erase(types::BREEZE_JUMP_INHALING.id());
        tick.brain.erase(types::BREEZE_LEAVING_WATER.id());
    }

    fn debug_name(&self) -> &'static str {
        "LongJump"
    }
}

const STUCK_SHOOT_WINDOW: i64 = 60;
const SHOOT_WHEN_STUCK_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::BREEZE_JUMP_INHALING.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `ShootWhenStuck`: riding, swimming or levitating, the breeze shoots instead.
struct ShootWhenStuck;

impl Behavior for ShootWhenStuck {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SHOOT_WHEN_STUCK_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        entity.has_vehicle()
            || entity.is_in_water()
            || tick
                .mob
                .get_mob_entity()
                .living_entity
                .has_effect(&StatusEffect::LEVITATION)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain
            .set_with_expiry(types::BREEZE_SHOOT, (), STUCK_SHOOT_WINDOW);
    }

    fn debug_name(&self) -> &'static str {
        "ShootWhenStuck"
    }
}

const SLIDE_AWAY_RANGE: i32 = 5;
const SLIDE_CONDITIONS: &[(MemoryModuleId, MemoryStatus)] = &[
    (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
    (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_JUMP_COOLDOWN.id(), MemoryStatus::ValueAbsent),
    (types::BREEZE_SHOOT.id(), MemoryStatus::ValueAbsent),
];

/// Vanilla `Slide`: repositions around the target between attacks.
struct Slide;

impl Slide {
    fn random_point_in_middle_circle(mob: &dyn Mob, enemy: &dyn EntityBase) -> Vector3<f64> {
        let body_pos = mob.get_entity().pos.load();
        let direction = enemy.get_entity().pos.load() - body_pos;
        let t: f64 = mob.get_random().random();
        let distance = direction.length()
            - t.mul_add(
                JUMP_CIRCLE_INNER_RADIUS - JUMP_CIRCLE_MIDDLE_RADIUS,
                JUMP_CIRCLE_MIDDLE_RADIUS,
            );
        body_pos + direction.normalize() * distance
    }
}

impl Behavior for Slide {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        SLIDE_CONDITIONS
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        on_ground(tick.mob)
            && !tick.mob.get_entity().is_in_water()
            && pose(tick.mob) == EntityPose::Standing
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(enemy) = tick.brain.get(types::ATTACK_TARGET).map(Arc::clone) else {
            return;
        };
        let enemy_pos = enemy.get_entity().pos.load();
        let body_pos = tick.mob.get_entity().pos.load();
        let away = if within_inner_circle_range(tick.mob, enemy_pos) {
            default_random_pos::get_pos_away(
                tick.mob,
                SLIDE_AWAY_RANGE,
                SLIDE_AWAY_RANGE,
                enemy_pos,
            )
            .filter(|away| {
                has_line_of_sight(tick, *away)
                    && enemy_pos.squared_distance_to_vec(away)
                        > enemy_pos.squared_distance_to_vec(&body_pos)
            })
        } else {
            None
        };
        let position = away.unwrap_or_else(|| {
            if tick.mob.get_random().random::<bool>() {
                random_point_behind_target(enemy.as_ref(), tick.mob)
            } else {
                Self::random_point_in_middle_circle(tick.mob, enemy.as_ref())
            }
        });
        tick.brain.set(
            types::WALK_TARGET,
            WalkTarget::from_block_pos(
                BlockPos::floored_v(position),
                SPEED_MULTIPLIER_WHEN_SLIDING,
                1,
            ),
        );
    }

    fn debug_name(&self) -> &'static str {
        "Slide"
    }
}

const MEMORY_TYPES: &[MemoryModuleId] = &[];

const SENSOR_TYPES: &[SensorType] = &[
    SensorType::NearestLivingEntities,
    SensorType::HurtBy,
    SensorType::NearestPlayers,
    SensorType::BreezeAttackEntity,
];

pub static BREEZE_PROVIDER: BrainProvider = BrainProvider {
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
