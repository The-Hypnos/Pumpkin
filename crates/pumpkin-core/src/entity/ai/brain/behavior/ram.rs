use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::ai::target_predicate::TargetPredicate;
use crate::entity::combat::knockback_after_resistance;
use crate::entity::mob::Mob;

use super::super::BrainTick;
use super::super::memory::position_tracker::{EntityTracker, PositionTracker};
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;
use super::utils::is_alive;

const PREPARE_TIME_OUT_DURATION: i32 = 160;
const RAM_TIME_OUT_DURATION: i32 = 200;
const RAM_SPEED_FORCE_FACTOR: f32 = 1.65;
const REACHED_RAM_TARGET_DISTANCE: f64 = 0.25;

fn walk_target_to(pos: BlockPos, speed: f32) -> WalkTarget {
    WalkTarget::from_block_pos(pos, speed, 0)
}

fn broadcast_event(tick: &BrainTick<'_>, status: EntityStatus) {
    tick.world
        .send_entity_status(tick.mob.get_entity(), status, None);
}

struct RamCandidate {
    start_position: BlockPos,
    target_position: BlockPos,
    target: Arc<dyn EntityBase>,
}

/// Vanilla `PrepareRamNearestTarget`: lines up a ram run against the nearest valid target.
pub struct PrepareRamNearestTarget {
    conditions: [(MemoryModuleId, MemoryStatus); 4],
    cooldown_on_fail: fn(&dyn Mob) -> i32,
    min_ram_distance: i32,
    max_ram_distance: i32,
    walk_speed: f32,
    ram_targeting: TargetPredicate,
    ram_prepare_time: i64,
    prepare_ram_sound: fn(&dyn Mob) -> Sound,
    reached_ram_position_timestamp: Option<i64>,
    ram_candidate: Option<RamCandidate>,
}

impl PrepareRamNearestTarget {
    #[must_use]
    pub const fn new(
        cooldown_on_fail: fn(&dyn Mob) -> i32,
        min_ram_distance: i32,
        max_ram_distance: i32,
        walk_speed: f32,
        ram_targeting: TargetPredicate,
        ram_prepare_time: i64,
        prepare_ram_sound: fn(&dyn Mob) -> Sound,
    ) -> Self {
        Self {
            conditions: [
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
                (
                    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                    MemoryStatus::ValuePresent,
                ),
                (types::RAM_TARGET.id(), MemoryStatus::ValueAbsent),
            ],
            cooldown_on_fail,
            min_ram_distance,
            max_ram_distance,
            walk_speed,
            ram_targeting,
            ram_prepare_time,
            prepare_ram_sound,
            reached_ram_position_timestamp: None,
            ram_candidate: None,
        }
    }

    /// Vanilla `isWalkableBlock`: a stable destination with no pathfinding penalty.
    fn is_walkable_block(mob: &dyn Mob, pos: &BlockPos) -> bool {
        let world = mob.get_entity().world.load_full();
        if !world.get_block_state(&pos.down()).is_solid_render() {
            return false;
        }
        let mut context =
            PathfindingContext::new(mob.get_entity().block_pos.load().0, Arc::clone(&world));
        let path_type = context.get_land_node_type(pos.0);
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_pathfinding_malus(path_type)
            == 0.0
    }

    fn calculate_ramming_start_position(
        &self,
        mob: &dyn Mob,
        target: &dyn EntityBase,
    ) -> Option<BlockPos> {
        let target_pos = target.get_entity().block_pos.load();
        if !Self::is_walkable_block(mob, &target_pos) {
            return None;
        }
        let mut possible_positions = Vec::new();
        for step in [
            BlockPos::north,
            BlockPos::east,
            BlockPos::south,
            BlockPos::west,
        ] {
            let mut furthest = target_pos;
            for _ in 0..self.max_ram_distance {
                let next = step(&furthest);
                if !Self::is_walkable_block(mob, &next) {
                    break;
                }
                furthest = next;
            }
            if furthest.manhattan_distance(target_pos) >= self.min_ram_distance {
                possible_positions.push(furthest);
            }
        }
        let body_pos = mob.get_entity().block_pos.load();
        possible_positions.sort_by_key(|pos| body_pos.squared_distance(pos));
        let mob_entity = mob.get_mob_entity();
        possible_positions.into_iter().find(|pos| {
            let destination = Vector3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y),
                f64::from(pos.0.z) + 0.5,
            );
            mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .create_path(&mob_entity.living_entity, destination, 0)
                .is_some_and(|path| path.can_reach())
        })
    }

    fn choose_ram_position(&mut self, mob: &dyn Mob, target: Arc<dyn EntityBase>) {
        self.reached_ram_position_timestamp = None;
        self.ram_candidate = self
            .calculate_ramming_start_position(mob, target.as_ref())
            .map(|start_position| RamCandidate {
                start_position,
                target_position: target.get_entity().block_pos.load(),
                target,
            });
    }

    /// Vanilla `getEdgeOfBlock`.
    fn get_edge_of_block(start: BlockPos, target: BlockPos) -> Vector3<f64> {
        let x_offset = 0.5 * f64::from((target.0.x - start.0.x).signum());
        let z_offset = 0.5 * f64::from((target.0.z - start.0.z).signum());
        Vector3::new(
            f64::from(target.0.x) + 0.5 + x_offset,
            f64::from(target.0.y),
            f64::from(target.0.z) + 0.5 + z_offset,
        )
    }
}

impl Behavior for PrepareRamNearestTarget {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn min_duration(&self) -> i32 {
        PREPARE_TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        PREPARE_TIME_OUT_DURATION
    }

    fn can_still_use(&mut self, _tick: &BrainTick<'_>) -> bool {
        self.ram_candidate
            .as_ref()
            .is_some_and(|candidate| is_alive(candidate.target.as_ref()))
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let target = {
            let ctx = tick.visibility();
            ctx.brain
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .and_then(|visible| {
                    visible.find_closest(&ctx, |entity| {
                        self.ram_targeting
                            .test(tick.world, Some(tick.mob), entity.as_ref())
                    })
                })
        };
        if let Some(target) = target {
            self.choose_ram_position(tick.mob, target);
        }
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(candidate) = &self.ram_candidate else {
            return;
        };
        let (start_position, target_position, target) = (
            candidate.start_position,
            candidate.target_position,
            Arc::clone(&candidate.target),
        );
        tick.brain.set(
            types::WALK_TARGET,
            walk_target_to(start_position, self.walk_speed),
        );
        tick.brain.set(
            types::LOOK_TARGET,
            Arc::new(EntityTracker::new(Arc::clone(&target), true)) as Arc<dyn PositionTracker>,
        );
        if target.get_entity().block_pos.load() != target_position {
            broadcast_event(tick, EntityStatus::EndRam);
            tick.mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            self.choose_ram_position(tick.mob, target);
            return;
        }
        if tick.mob.get_entity().block_pos.load() != start_position {
            return;
        }
        broadcast_event(tick, EntityStatus::StartRam);
        let reached = *self.reached_ram_position_timestamp.get_or_insert(tick.time);
        if tick.time - reached >= self.ram_prepare_time {
            tick.brain.set(
                types::RAM_TARGET,
                Self::get_edge_of_block(start_position, target_position),
            );
            let entity = tick.mob.get_entity();
            let base_pitch = if crate::entity::ageable::is_baby(tick.mob) {
                1.5
            } else {
                1.0
            };
            let mut rng = tick.mob.get_random();
            let pitch = (rng.random::<f32>() - rng.random::<f32>()).mul_add(0.2, base_pitch);
            tick.world.play_sound_fine(
                (self.prepare_ram_sound)(tick.mob),
                SoundCategory::Neutral,
                &entity.pos.load(),
                1.0,
                pitch,
            );
            self.ram_candidate = None;
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        if !tick.brain.has_memory_value(types::RAM_TARGET.id()) {
            broadcast_event(tick, EntityStatus::EndRam);
            let cooldown = (self.cooldown_on_fail)(tick.mob);
            tick.brain.set(types::RAM_COOLDOWN_TICKS, cooldown);
        }
    }

    fn debug_name(&self) -> &'static str {
        "PrepareRamNearestTarget"
    }
}

/// Vanilla `RamTarget`: charges at `RAM_TARGET`, knocking back what it hits.
pub struct RamTarget {
    conditions: [(MemoryModuleId, MemoryStatus); 2],
    time_between_rams: fn(&dyn Mob) -> (i32, i32),
    ram_targeting: TargetPredicate,
    speed: f32,
    knockback_force: fn(&dyn Mob) -> f64,
    impact_sound: fn(&dyn Mob) -> Sound,
    horn_break_sound: fn(&dyn Mob) -> Sound,
    drop_horn: fn(&dyn Mob) -> bool,
    ram_direction: Vector3<f64>,
}

impl RamTarget {
    #[must_use]
    pub const fn new(
        time_between_rams: fn(&dyn Mob) -> (i32, i32),
        ram_targeting: TargetPredicate,
        speed: f32,
        knockback_force: fn(&dyn Mob) -> f64,
        impact_sound: fn(&dyn Mob) -> Sound,
        horn_break_sound: fn(&dyn Mob) -> Sound,
        drop_horn: fn(&dyn Mob) -> bool,
    ) -> Self {
        Self {
            conditions: [
                (types::RAM_COOLDOWN_TICKS.id(), MemoryStatus::ValueAbsent),
                (types::RAM_TARGET.id(), MemoryStatus::ValuePresent),
            ],
            time_between_rams,
            ram_targeting,
            speed,
            knockback_force,
            impact_sound,
            horn_break_sound,
            drop_horn,
            ram_direction: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    fn play_sound(tick: &BrainTick<'_>, sound: Sound) {
        tick.world.play_sound_fine(
            sound,
            SoundCategory::Neutral,
            &tick.mob.get_entity().pos.load(),
            1.0,
            1.0,
        );
    }

    fn finish_ram(&self, tick: &mut BrainTick<'_>) {
        broadcast_event(tick, EntityStatus::EndRam);
        let (min, max) = (self.time_between_rams)(tick.mob);
        let cooldown = tick.mob.get_random().random_range(min..=max);
        tick.brain.set(types::RAM_COOLDOWN_TICKS, cooldown);
        tick.brain.erase(types::RAM_TARGET.id());
    }

    fn has_rammed_horn_breaking_block(tick: &BrainTick<'_>) -> bool {
        let entity = tick.mob.get_entity();
        let direction = entity.velocity.load().multiply(1.0, 0.0, 1.0).normalize();
        let facing = BlockPos::floored_v(entity.pos.load() + direction);
        let snaps = &tag::Block::MINECRAFT_SNAPS_GOAT_HORN;
        tick.world.get_block(&facing).has_tag(snaps)
            || tick.world.get_block(&facing.up()).has_tag(snaps)
    }

    fn effect_level(mob: &dyn Mob, effect: &'static StatusEffect) -> i32 {
        mob.get_mob_entity()
            .living_entity
            .get_effect(effect)
            .map_or(0, |effect| i32::from(effect.amplifier) + 1)
    }

    fn ram(&self, tick: &BrainTick<'_>, target: &dyn EntityBase) {
        let body = tick.mob;
        let living = &body.get_mob_entity().living_entity;
        let damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
        target.damage_with_context(
            target,
            damage,
            DamageType::MOB_ATTACK_NO_AGGRO,
            None,
            Some(body as &dyn EntityBase),
            Some(body as &dyn EntityBase),
        );
        let speed_boost_power = 0.25
            * (Self::effect_level(body, &StatusEffect::SPEED)
                - Self::effect_level(body, &StatusEffect::SLOWNESS)) as f32;
        // Vanilla `Mob.getSpeed`, the speed the move control last set.
        let current_speed = living.movement_input.load().z as f32;
        let speed_factor =
            (current_speed * RAM_SPEED_FORCE_FACTOR).clamp(0.2, 3.0) + speed_boost_power;
        let blocking = target
            .get_living_entity()
            .is_some_and(crate::entity::living::LivingEntity::is_blocking);
        let blocking_factor = if blocking { 0.5 } else { 1.0 };
        let Some(target_living) = target.get_living_entity() else {
            return;
        };
        let strength = knockback_after_resistance(
            f64::from(blocking_factor * speed_factor) * (self.knockback_force)(body),
            target_living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE),
        );
        target
            .get_entity()
            .apply_knockback(strength, self.ram_direction.x, self.ram_direction.z);
    }
}

impl Behavior for RamTarget {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn min_duration(&self) -> i32 {
        RAM_TIME_OUT_DURATION
    }

    fn max_duration(&self) -> i32 {
        RAM_TIME_OUT_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::RAM_TARGET.id())
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.has_memory_value(types::RAM_TARGET.id())
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(ram_target) = tick.brain.get(types::RAM_TARGET).copied() else {
            return;
        };
        let current = tick.mob.get_entity().block_pos.load();
        self.ram_direction = Vector3::new(
            f64::from(current.0.x) - ram_target.x,
            0.0,
            f64::from(current.0.z) - ram_target.z,
        )
        .normalize();
        tick.brain.set(
            types::WALK_TARGET,
            WalkTarget::from_vec(ram_target, self.speed, 0),
        );
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let body_box = tick.mob.get_entity().bounding_box.load();
        let hit = tick
            .world
            .entity_grid
            .load()
            .collect_in_box(&body_box)
            .into_iter()
            .find(|entity| {
                entity.get_living_entity().is_some()
                    && self
                        .ram_targeting
                        .test(tick.world, Some(tick.mob), entity.as_ref())
            });
        if let Some(target) = hit {
            self.ram(tick, target.as_ref());
            self.finish_ram(tick);
            Self::play_sound(tick, (self.impact_sound)(tick.mob));
        } else if Self::has_rammed_horn_breaking_block(tick) {
            Self::play_sound(tick, (self.impact_sound)(tick.mob));
            if (self.drop_horn)(tick.mob) {
                Self::play_sound(tick, (self.horn_break_sound)(tick.mob));
            }
            self.finish_ram(tick);
        } else {
            let lost_or_reached = match (
                tick.brain.get(types::WALK_TARGET),
                tick.brain.get(types::RAM_TARGET),
            ) {
                (Some(walk_target), Some(ram_target)) => {
                    walk_target
                        .target()
                        .current_position()
                        .squared_distance_to_vec(ram_target)
                        < REACHED_RAM_TARGET_DISTANCE * REACHED_RAM_TARGET_DISTANCE
                }
                _ => true,
            };
            if lost_or_reached {
                self.finish_ram(tick);
            }
        }
    }

    fn debug_name(&self) -> &'static str {
        "RamTarget"
    }
}
