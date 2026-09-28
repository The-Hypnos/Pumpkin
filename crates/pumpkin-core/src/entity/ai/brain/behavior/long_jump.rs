use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityPose;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{Tag, Taggable};
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::seq::SliceRandom;

use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::mob::Mob;

use super::super::BrainTick;
use super::super::memory::position_tracker::{BlockPosTracker, PositionTracker};
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;

const FIND_JUMP_TRIES: i32 = 20;
const PREPARE_JUMP_DURATION: i64 = 40;
const TIME_OUT_DURATION: i32 = 200;
/// `pickCandidate` only treats a spot as walkable if a path within this length reaches it.
const WALK_PATH_MAX_LENGTH: f32 = 8.0;
const MID_JUMP_TIME_OUT_DURATION: i32 = 100;
const ALLOWED_ANGLES: [i32; 4] = [65, 70, 75, 80];

pub type LandingSpotCheck = fn(&dyn Mob, &BlockPos) -> bool;

fn sample(range: (i32, i32), mob: &dyn Mob) -> i32 {
    mob.get_random().random_range(range.0..=range.1)
}

/// Vanilla `LongJumpMidJump`: holds the jump pose until the mob lands.
pub struct LongJumpMidJump {
    conditions: [(MemoryModuleId, MemoryStatus); 2],
    time_between_long_jumps: (i32, i32),
    landing_sound: Sound,
}

impl LongJumpMidJump {
    #[must_use]
    pub const fn new(time_between_long_jumps: (i32, i32), landing_sound: Sound) -> Self {
        Self {
            conditions: [
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValuePresent),
            ],
            time_between_long_jumps,
            landing_sound,
        }
    }
}

impl Behavior for LongJumpMidJump {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn min_duration(&self) -> i32 {
        MID_JUMP_TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        MID_JUMP_TIME_OUT_DURATION
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        !tick
            .mob
            .get_entity()
            .on_ground
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let living = &tick.mob.get_mob_entity().living_entity;
        living
            .discard_friction
            .store(true, std::sync::atomic::Ordering::Relaxed);
        living.entity.set_pose(EntityPose::LongJumping);
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let living = &tick.mob.get_mob_entity().living_entity;
        let entity = &living.entity;
        if entity.on_ground.load(std::sync::atomic::Ordering::Relaxed) {
            entity
                .velocity
                .store(entity.velocity.load().multiply(0.1, 1.0, 0.1));
            tick.world.play_sound_fine(
                self.landing_sound,
                SoundCategory::Neutral,
                &entity.pos.load(),
                2.0,
                1.0,
            );
        }
        living
            .discard_friction
            .store(false, std::sync::atomic::Ordering::Relaxed);
        entity.set_pose(EntityPose::Standing);
        tick.brain.erase(types::LONG_JUMP_MID_JUMP.id());
        let cooldown = sample(self.time_between_long_jumps, tick.mob);
        tick.brain.set(types::LONG_JUMP_COOLDOWN_TICKS, cooldown);
    }

    fn debug_name(&self) -> &'static str {
        "LongJumpMidJump"
    }
}

#[derive(Clone, Copy)]
struct PossibleJump {
    target_pos: BlockPos,
    weight: i32,
}

/// Vanilla `LongJumpToRandomPos`, and `LongJumpToPreferredBlock` when `preferred` is set.
pub struct LongJumpToRandomPos {
    conditions: [(MemoryModuleId, MemoryStatus); 3],
    time_between_long_jumps: (i32, i32),
    max_long_jump_height: i32,
    max_long_jump_width: i32,
    max_jump_velocity_multiplier: f32,
    jump_sound: fn(&dyn Mob) -> Sound,
    acceptable_landing_spot: LandingSpotCheck,
    preferred: Option<(&'static Tag, f32)>,
    /// Vanilla sizes the jump arc check with `getDimensions(Pose.LONG_JUMPING)`.
    jump_dimensions_scale: f32,
    jump_candidates: Vec<PossibleJump>,
    not_preferred_jump_candidates: Vec<PossibleJump>,
    currently_wanting_preferred_ones: bool,
    initial_position: Option<Vector3<f64>>,
    chosen_jump: Option<Vector3<f64>>,
    find_jump_tries: i32,
    prepare_jump_start: i64,
}

impl LongJumpToRandomPos {
    #[must_use]
    pub fn new(
        time_between_long_jumps: (i32, i32),
        max_long_jump_height: i32,
        max_long_jump_width: i32,
        max_jump_velocity_multiplier: f32,
        jump_sound: fn(&dyn Mob) -> Sound,
        acceptable_landing_spot: LandingSpotCheck,
    ) -> Self {
        Self {
            conditions: [
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (
                    types::LONG_JUMP_COOLDOWN_TICKS.id(),
                    MemoryStatus::ValueAbsent,
                ),
                (types::LONG_JUMP_MID_JUMP.id(), MemoryStatus::ValueAbsent),
            ],
            time_between_long_jumps,
            max_long_jump_height,
            max_long_jump_width,
            max_jump_velocity_multiplier,
            jump_sound,
            acceptable_landing_spot,
            preferred: None,
            jump_dimensions_scale: 1.0,
            jump_candidates: Vec::new(),
            not_preferred_jump_candidates: Vec::new(),
            currently_wanting_preferred_ones: false,
            initial_position: None,
            chosen_jump: None,
            find_jump_tries: 0,
            prepare_jump_start: 0,
        }
    }

    /// For mobs whose long-jumping pose is smaller, like the goat's `scale(0.7F)`.
    #[must_use]
    pub const fn with_jump_dimensions_scale(mut self, scale: f32) -> Self {
        self.jump_dimensions_scale = scale;
        self
    }

    /// Vanilla `LongJumpToPreferredBlock`: with `chance`, favours landing on `tag`.
    #[must_use]
    pub const fn preferring(mut self, tag: &'static Tag, chance: f32) -> Self {
        self.preferred = Some((tag, chance));
        self
    }

    fn set_half_cooldown(&self, tick: &mut BrainTick<'_>) {
        let cooldown = sample(self.time_between_long_jumps, tick.mob) / 2;
        tick.brain.set(types::LONG_JUMP_COOLDOWN_TICKS, cooldown);
    }

    /// Vanilla `WeightedRandom.getRandomItem`, removing the pick.
    fn take_weighted_candidate(&mut self, mob: &dyn Mob) -> Option<PossibleJump> {
        let total: i32 = self.jump_candidates.iter().map(|jump| jump.weight).sum();
        if total <= 0 {
            return None;
        }
        let mut selection = mob.get_random().random_range(0..total);
        let index = self.jump_candidates.iter().position(|jump| {
            selection -= jump.weight;
            selection < 0
        })?;
        Some(self.jump_candidates.remove(index))
    }

    fn get_jump_candidate(&mut self, tick: &BrainTick<'_>) -> Option<PossibleJump> {
        let Some((preferred_tag, _)) = self.preferred else {
            return self.take_weighted_candidate(tick.mob);
        };
        if !self.currently_wanting_preferred_ones {
            return self.take_weighted_candidate(tick.mob);
        }
        while !self.jump_candidates.is_empty() {
            if let Some(jump) = self.take_weighted_candidate(tick.mob) {
                if tick
                    .world
                    .get_block(&jump.target_pos.down())
                    .has_tag(preferred_tag)
                {
                    return Some(jump);
                }
                self.not_preferred_jump_candidates.push(jump);
            }
        }
        (!self.not_preferred_jump_candidates.is_empty())
            .then(|| self.not_preferred_jump_candidates.remove(0))
    }

    fn pick_candidate(&mut self, tick: &mut BrainTick<'_>) {
        while !self.jump_candidates.is_empty() {
            let Some(jump) = self.get_jump_candidate(tick) else {
                continue;
            };
            let target_pos = jump.target_pos;
            if !self.is_acceptable_landing_position(tick.mob, &target_pos) {
                continue;
            }
            let target = target_pos.to_centered_f64();
            let Some(jump_vector) = self.calculate_optimal_jump_vector(tick.mob, target) else {
                continue;
            };
            tick.brain.set(
                types::LOOK_TARGET,
                Arc::new(BlockPosTracker::new(target_pos)) as Arc<dyn PositionTracker>,
            );
            let mob_entity = tick.mob.get_mob_entity();
            let reachable = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .create_path_within(
                    &mob_entity.living_entity,
                    target_pos,
                    0,
                    WALK_PATH_MAX_LENGTH,
                )
                .is_some_and(|path| path.can_reach());
            if !reachable {
                self.chosen_jump = Some(jump_vector);
                self.prepare_jump_start = tick.time;
                return;
            }
        }
    }

    fn is_acceptable_landing_position(&self, mob: &dyn Mob, target_pos: &BlockPos) -> bool {
        let body_pos = mob.get_entity().block_pos.load();
        if body_pos.0.x == target_pos.0.x && body_pos.0.z == target_pos.0.z {
            return false;
        }
        (self.acceptable_landing_spot)(mob, target_pos)
    }

    fn calculate_optimal_jump_vector(
        &self,
        mob: &dyn Mob,
        target_pos: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let mut allowed_angles = ALLOWED_ANGLES;
        allowed_angles.shuffle(&mut mob.get_random());
        let max_jump_velocity = (mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::JUMP_STRENGTH)
            * f64::from(self.max_jump_velocity_multiplier)) as f32;
        allowed_angles.into_iter().find_map(|angle| {
            calculate_jump_vector_for_angle(
                mob,
                target_pos,
                max_jump_velocity,
                angle,
                true,
                self.jump_dimensions_scale,
            )
        })
    }
}

impl Behavior for LongJumpToRandomPos {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn min_duration(&self) -> i32 {
        TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        TIME_OUT_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        let can_start = entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
            && !entity.is_in_water()
            && !entity
                .touching_lava
                .load(std::sync::atomic::Ordering::Relaxed)
            && tick.world.get_block(&entity.block_pos.load()).id != Block::HONEY_BLOCK.id;
        if !can_start {
            self.set_half_cooldown(tick);
        }
        can_start
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        self.initial_position == Some(entity.pos.load())
            && self.find_jump_tries > 0
            && !entity.is_in_water()
            && (self.chosen_jump.is_some() || !self.jump_candidates.is_empty())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        self.chosen_jump = None;
        self.find_jump_tries = FIND_JUMP_TRIES;
        self.initial_position = Some(tick.mob.get_entity().pos.load());
        let mob_pos = tick.mob.get_entity().block_pos.load();
        let (width, height) = (self.max_long_jump_width, self.max_long_jump_height);
        self.jump_candidates.clear();
        // `BlockPos.betweenClosed` order (x fastest), which the weighted pick depends on.
        for z in -width..=width {
            for y in -height..=height {
                for x in -width..=width {
                    if x == 0 && y == 0 && z == 0 {
                        continue;
                    }
                    let target_pos = mob_pos.add(x, y, z);
                    self.jump_candidates.push(PossibleJump {
                        target_pos,
                        weight: f64::from(mob_pos.squared_distance(&target_pos)).ceil() as i32,
                    });
                }
            }
        }
        if let Some((_, chance)) = self.preferred {
            self.not_preferred_jump_candidates.clear();
            self.currently_wanting_preferred_ones = tick.mob.get_random().random::<f32>() < chance;
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(chosen_jump) = self.chosen_jump {
            if tick.time - self.prepare_jump_start >= PREPARE_JUMP_DURATION {
                let living = &tick.mob.get_mob_entity().living_entity;
                let entity = &living.entity;
                entity.set_rotation(entity.body_yaw.load(), entity.pitch.load());
                living
                    .discard_friction
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let original_length = chosen_jump.length();
                let length_with_jump_boost = original_length + living.get_jump_boost_power();
                entity.set_velocity(chosen_jump * (length_with_jump_boost / original_length));
                tick.brain.set(types::LONG_JUMP_MID_JUMP, true);
                tick.world.play_sound_fine(
                    (self.jump_sound)(tick.mob),
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                    1.0,
                    1.0,
                );
            }
        } else {
            self.find_jump_tries -= 1;
            self.pick_candidate(tick);
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        // Vanilla does this in `canStillUse` when it turns false; `stop` always follows.
        if !tick.brain.has_memory_value(types::LONG_JUMP_MID_JUMP.id()) {
            self.set_half_cooldown(tick);
            tick.brain.erase(types::LOOK_TARGET.id());
        }
    }

    fn debug_name(&self) -> &'static str {
        "LongJumpToRandomPos"
    }
}

/// Vanilla `LongJumpToRandomPos.defaultAcceptableLandingSpot`.
#[must_use]
pub fn default_acceptable_landing_spot(mob: &dyn Mob, target_pos: &BlockPos) -> bool {
    let world = mob.get_entity().world.load_full();
    if !world.get_block_state(&target_pos.down()).is_solid_render() {
        return false;
    }
    let mut context = PathfindingContext::new(mob.get_entity().block_pos.load().0, world);
    let path_type = context.get_land_node_type(target_pos.0);
    mob.get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_pathfinding_malus(path_type)
        == 0.0
}

/// Vanilla `LongJumpUtil.calculateJumpVectorForAngle`.
pub fn calculate_jump_vector_for_angle(
    mob: &dyn Mob,
    target_pos: Vector3<f64>,
    max_jump_velocity: f32,
    angle: i32,
    check_collision: bool,
    jump_dimensions_scale: f32,
) -> Option<Vector3<f64>> {
    let mob_pos = mob.get_entity().pos.load();
    let direction_plane =
        Vector3::new(target_pos.x - mob_pos.x, 0.0, target_pos.z - mob_pos.z).normalize() * 0.5;
    let target_position = target_pos - direction_plane;
    let direction = target_position - mob_pos;
    let angle_rad = angle as f32 * std::f32::consts::PI / 180.0;
    let xz_angle = direction.z.atan2(direction.x);
    let r2 = direction.x.mul_add(direction.x, direction.z * direction.z);
    let r = r2.sqrt();
    let y = direction.y;
    let gravity = mob.get_gravity();
    let sin_2_angle = f64::from(2.0 * angle_rad).sin();
    let cos_angle_sqr = f64::from(angle_rad).cos().powi(2);
    let sin_angle = f64::from(angle_rad).sin();
    let cos_angle = f64::from(angle_rad).cos();
    let sin_xz_angle = xz_angle.sin();
    let cos_xz_angle = xz_angle.cos();
    let v0_sqr = r2 * gravity / r.mul_add(sin_2_angle, -(2.0 * y * cos_angle_sqr));
    if v0_sqr < 0.0 {
        return None;
    }
    let v0 = v0_sqr.sqrt();
    if v0 > f64::from(max_jump_velocity) {
        return None;
    }
    let v0_r = v0 * cos_angle;
    let v0_y = v0 * sin_angle;
    if check_collision {
        let samples = (r / v0_r).ceil() as i32 * 2;
        let unscaled = mob.get_entity().entity_dimension.load();
        let dimensions = EntityDimensions {
            width: unscaled.width * jump_dimensions_scale,
            height: unscaled.height * jump_dimensions_scale,
            eye_height: unscaled.eye_height * jump_dimensions_scale,
        };
        let mut ri = 0.0;
        let mut previous: Option<Vector3<f64>> = None;
        for _ in 0..samples - 1 {
            ri += r / f64::from(samples);
            let yi = sin_angle / cos_angle * ri
                - ri.powi(2) * gravity / (2.0 * v0_sqr * cos_angle.powi(2));
            let sample = Vector3::new(
                ri.mul_add(cos_xz_angle, mob_pos.x),
                mob_pos.y + yi,
                ri.mul_add(sin_xz_angle, mob_pos.z),
            );
            if let Some(previous) = previous
                && !is_clear_transition(mob, &dimensions, previous, sample)
            {
                return None;
            }
            previous = Some(sample);
        }
    }
    Some(Vector3::new(v0_r * cos_xz_angle, v0_y, v0_r * sin_xz_angle) * f64::from(0.95f32))
}

fn is_clear_transition(
    mob: &dyn Mob,
    dimensions: &EntityDimensions,
    from: Vector3<f64>,
    to: Vector3<f64>,
) -> bool {
    let direction = to - from;
    let min_dimension = f64::from(dimensions.width.min(dimensions.height));
    let checks = (direction.length() / min_dimension).ceil() as i32;
    let normalized = direction.normalize();
    let world = mob.get_entity().world.load();
    let mut next = from;
    for i in 0..checks {
        next = if i == checks - 1 {
            to
        } else {
            next + normalized * (min_dimension * f64::from(0.9f32))
        };
        if !world.is_space_empty(BoundingBox::new_from_pos(
            next.x, next.y, next.z, dimensions,
        )) {
            return false;
        }
    }
    true
}
