use std::sync::Arc;

use pumpkin_data::tag::Tag;
use pumpkin_data::{BlockDirection, BlockStateId};
use pumpkin_util::math::position::BlockPos;

use crate::entity::ai::brain::sensing::find_first_in_box_by_manhattan_distance;
use crate::entity::ai::util::goal_utils::{fluid_has_tag, is_water_state};
use crate::world::World;

use super::super::BrainTick;
use super::super::memory::position_tracker::{BlockPosTracker, PositionTracker};
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryStatus, types};
use super::one_shot::OneShot;

const TRY_FIND_LAND_COOLDOWN_TICKS: i64 = 60;
const TRY_FIND_LAND_NEAR_LIQUID_COOLDOWN_TICKS: i64 = 40;
const TRY_FIND_LIQUID_RETRY_TICKS: i64 = 22;
const TRY_FIND_LIQUID_COOLDOWN_TICKS: i64 = 40;
const TOO_CLOSE_TO_LIQUID: f64 = 1.5;

fn has_fluid(state_id: BlockStateId) -> bool {
    state_id.to_state().is_waterlogged()
        || pumpkin_data::fluid::Fluid::from_state_id(state_id).is_some()
}

fn has_no_collision(world: &World, pos: &BlockPos) -> bool {
    world
        .get_block_state(pos)
        .get_block_collision_shapes_at(pos)
        .next()
        .is_none()
}

fn state_at(world: &World, pos: &BlockPos) -> Option<BlockStateId> {
    world.get_block_state_id_if_loaded(pos)
}

fn conditions() -> Vec<(super::super::memory::MemoryModuleId, MemoryStatus)> {
    vec![
        (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
        (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
        (types::LOOK_TARGET.id(), MemoryStatus::Registered),
    ]
}

fn walk_and_look_to(
    tick: &mut BrainTick<'_>,
    target: BlockPos,
    speed_modifier: f32,
    close_enough: i32,
) {
    let tracker: Arc<dyn PositionTracker> = Arc::new(BlockPosTracker::new(target));
    tick.brain.set(types::LOOK_TARGET, Arc::clone(&tracker));
    tick.brain.set(
        types::WALK_TARGET,
        WalkTarget::new(tracker, speed_modifier, close_enough),
    );
}

/// Vanilla `TryFindLand`: from water, walk to the nearest dry spot with a sturdy floor.
#[must_use]
pub fn try_find_land(range: i32, speed_modifier: f32) -> OneShot {
    let mut next_ok_start_time = 0i64;
    OneShot::new("TryFindLand", conditions(), move |tick| {
        let body_pos = tick.mob.get_entity().block_pos.load();
        if !state_at(tick.world, &body_pos).is_some_and(is_water_state) {
            return false;
        }
        if tick.time < next_ok_start_time {
            next_ok_start_time = tick.time + TRY_FIND_LAND_COOLDOWN_TICKS;
            return true;
        }
        let world = Arc::clone(tick.world);
        let land = find_first_in_box_by_manhattan_distance(body_pos, range, range, |pos| {
            if pos.0.x == body_pos.0.x && pos.0.z == body_pos.0.z {
                return false;
            }
            let Some(state_id) = state_at(&world, pos) else {
                return false;
            };
            !has_fluid(state_id)
                && has_no_collision(&world, pos)
                && world
                    .get_block_state(&pos.down())
                    .is_side_solid(BlockDirection::Up)
        });
        if let Some(target) = land {
            walk_and_look_to(tick, target, speed_modifier, 1);
        }
        next_ok_start_time = tick.time + TRY_FIND_LAND_COOLDOWN_TICKS;
        true
    })
}

/// Vanilla `TryFindLandNearLiquid`: walk to an open spot on the shore of `fluid_tag`.
#[must_use]
pub fn try_find_land_near_liquid(
    range: i32,
    speed_modifier: f32,
    fluid_tag: &'static Tag,
) -> OneShot {
    let mut next_ok_start_time = 0i64;
    OneShot::new("TryFindLandNearLiquid", conditions(), move |tick| {
        let body_pos = tick.mob.get_entity().block_pos.load();
        if state_at(tick.world, &body_pos).is_some_and(|state| fluid_has_tag(state, fluid_tag)) {
            return false;
        }
        if tick.time < next_ok_start_time {
            next_ok_start_time = tick.time + TRY_FIND_LAND_NEAR_LIQUID_COOLDOWN_TICKS;
            return true;
        }
        let world = Arc::clone(tick.world);
        let land = find_first_in_box_by_manhattan_distance(body_pos, range, range, |pos| {
            if pos.0.x == body_pos.0.x && pos.0.z == body_pos.0.z {
                return false;
            }
            // Vanilla tests the block below against the collision context of `pos`.
            if !has_no_collision(&world, pos) || has_no_collision(&world, &pos.down()) {
                return false;
            }
            [pos.north(), pos.east(), pos.south(), pos.west()]
                .iter()
                .any(|side| {
                    world.get_block_state(side).is_air()
                        && state_at(&world, &side.down())
                            .is_some_and(|state| fluid_has_tag(state, fluid_tag))
                })
        });
        if let Some(target) = land {
            walk_and_look_to(tick, target, speed_modifier, 0);
        }
        next_ok_start_time = tick.time + TRY_FIND_LAND_NEAR_LIQUID_COOLDOWN_TICKS;
        true
    })
}

/// Vanilla `TryFindLiquid`: from land, walk to the nearest `fluid_tag` source with air above,
/// or failing that the nearest one not right underfoot.
#[must_use]
pub fn try_find_liquid(range: i32, speed_modifier: f32, fluid_tag: &'static Tag) -> OneShot {
    let mut next_ok_start_time = 0i64;
    OneShot::new("TryFindLiquid", conditions(), move |tick| {
        let body_pos = tick.mob.get_entity().block_pos.load();
        if state_at(tick.world, &body_pos).is_some_and(|state| fluid_has_tag(state, fluid_tag)) {
            return false;
        }
        if tick.time < next_ok_start_time {
            next_ok_start_time = tick.time + TRY_FIND_LIQUID_RETRY_TICKS;
            return true;
        }
        let world = Arc::clone(tick.world);
        let position = tick.mob.get_entity().pos.load();
        let mut found: Option<BlockPos> = None;
        find_first_in_box_by_manhattan_distance(body_pos, range, range, |pos| {
            if pos.0.x == body_pos.0.x && pos.0.z == body_pos.0.z {
                return false;
            }
            let Some(state_id) = state_at(&world, pos) else {
                return false;
            };
            let block = pumpkin_data::Block::from_state_id(state_id);
            let is_liquid_block = block.id == pumpkin_data::Block::WATER.id
                || block.id == pumpkin_data::Block::LAVA.id;
            if !is_liquid_block || !fluid_has_tag(state_id, fluid_tag) {
                return false;
            }
            if world.get_block_state(&pos.up()).is_air() {
                found = Some(*pos);
                return true;
            }
            let too_close = pos.to_centered_f64().squared_distance_to_vec(&position)
                < TOO_CLOSE_TO_LIQUID * TOO_CLOSE_TO_LIQUID;
            if found.is_none() && !too_close {
                found = Some(*pos);
            }
            false
        });
        if let Some(target) = found {
            walk_and_look_to(tick, target, speed_modifier, 0);
        }
        next_ok_start_time = tick.time + TRY_FIND_LIQUID_COOLDOWN_TICKS;
        true
    })
}
