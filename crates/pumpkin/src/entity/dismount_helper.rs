//! Vanilla `DismountHelper`: where an entity can stand after leaving a vehicle or a bed.

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::world::World;
use crate::world::natural_spawner::is_block_dangerous;

/// Vanilla `nonClimbableShape`, reduced to the top of the shape: `None` for ladders, open
/// trapdoors and blocks without collision.
fn non_climbable_top(world: &World, pos: &BlockPos) -> Option<f64> {
    let (block, state) = world.get_block_and_state(pos);
    if block.has_tag(&tag::Block::MINECRAFT_CLIMBABLE)
        || (block.has_tag(&tag::Block::MINECRAFT_TRAPDOORS) && is_open_trapdoor(state.id))
    {
        return None;
    }
    state
        .get_block_collision_shapes_at(pos)
        .map(|shape| shape.max.y)
        .reduce(f64::max)
}

fn is_open_trapdoor(state: pumpkin_data::BlockStateId) -> bool {
    use pumpkin_data::block_properties::OakTrapdoorLikeProperties;
    OakTrapdoorLikeProperties::from_state_id(state).open
}

/// Vanilla `CollisionGetter.getBlockFloorHeight` with the non-climbable shapes.
fn block_floor_height(world: &World, pos: &BlockPos) -> f64 {
    if let Some(top) = non_climbable_top(world, pos) {
        return top;
    }
    match non_climbable_top(world, &pos.down()) {
        Some(below) if below >= 1.0 => below - 1.0,
        _ => f64::NEG_INFINITY,
    }
}

/// Vanilla `isBlockFloorValid`.
fn is_block_floor_valid(floor_height: f64) -> bool {
    floor_height.is_finite() && floor_height < 1.0
}

/// Vanilla `DismountHelper.findSafeDismountLocation`.
#[must_use]
pub fn find_safe_dismount_location(
    entity_type: &'static EntityType,
    world: &World,
    pos: &BlockPos,
    check_dangerous: bool,
) -> Option<Vector3<f64>> {
    if check_dangerous && is_block_dangerous(entity_type, world.get_block_state(pos)) {
        return None;
    }
    let floor_height = block_floor_height(world, pos);
    if !is_block_floor_valid(floor_height) {
        return None;
    }
    if check_dangerous
        && floor_height <= 0.0
        && is_block_dangerous(entity_type, world.get_block_state(&pos.down()))
    {
        return None;
    }
    let position = Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y) + floor_height,
        f64::from(pos.0.z) + 0.5,
    );
    let dimensions = EntityDimensions {
        width: entity_type.dimension[0],
        height: entity_type.dimension[1],
        eye_height: entity_type.eye_height,
    };
    let aabb = BoundingBox::new_from_pos(position.x, position.y, position.z, &dimensions);
    if !world.is_space_empty(aabb) {
        return None;
    }
    // Vanilla also refuses spots inside `invalid_spawn_inside` blocks, but only for players.
    let border = world
        .worldborder
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (border.contains_block(aabb.min.x.floor() as i32, aabb.min.z.floor() as i32)
        && border.contains_block(aabb.max.x.floor() as i32, aabb.max.z.floor() as i32))
    .then_some(position)
}
