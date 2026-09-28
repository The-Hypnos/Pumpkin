use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use crate::entity::ai::util::default_random_pos;
use crate::entity::item::ItemEntity;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

use super::super::memory::position_tracker::{BlockPosTracker, EntityTracker, PositionTracker};
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleType, types};
use super::super::{Brain, VisibilityContext};

/// Vanilla `LivingEntity.isAlive`, health aware unlike `Entity::is_alive`.
#[must_use]
pub fn is_alive(entity: &dyn EntityBase) -> bool {
    entity.get_entity().is_alive()
        && entity
            .get_living_entity()
            .is_none_or(|living| living.health.load() > 0.0)
}

#[must_use]
pub fn is_dead_or_dying(entity: &dyn EntityBase) -> bool {
    entity
        .get_living_entity()
        .is_some_and(|living| living.health.load() <= 0.0)
}

#[must_use]
pub fn has_passenger(body: &crate::entity::Entity, target: &dyn EntityBase) -> bool {
    let target_id = target.get_entity().entity_id;
    body.passengers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|passenger| passenger.get_entity().entity_id == target_id)
}

#[must_use]
pub fn entity_is_visible(ctx: &VisibilityContext<'_>, target: &dyn EntityBase) -> bool {
    ctx.brain
        .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
        .is_some_and(|visible| visible.contains(target, ctx))
}

#[must_use]
pub fn can_see(ctx: &VisibilityContext<'_>, target: &dyn EntityBase) -> bool {
    entity_is_visible(ctx, target)
}

#[must_use]
pub fn target_is_valid(
    ctx: &VisibilityContext<'_>,
    memory: MemoryModuleType<Arc<dyn EntityBase>>,
    predicate: impl Fn(&Arc<dyn EntityBase>) -> bool,
) -> bool {
    ctx.brain.get(memory).is_some_and(|target| {
        predicate(target) && is_alive(target.as_ref()) && entity_is_visible(ctx, target.as_ref())
    })
}

pub fn look_at_entity(brain: &mut Brain, target: Arc<dyn EntityBase>) {
    brain.set(
        types::LOOK_TARGET,
        Arc::new(EntityTracker::new(target, true)) as Arc<dyn PositionTracker>,
    );
}

/// Vanilla `BehaviorUtils.throwItem`: tosses `item` from hand height toward `target`.
pub fn throw_item(
    thrower: &dyn EntityBase,
    item: ItemStack,
    target: Vector3<f64>,
    throw_velocity: Vector3<f64>,
    hand_y_distance_from_eye: f32,
) {
    let entity = thrower.get_entity();
    let world = entity.world.load_full();
    let pos = entity.pos.load();
    let hand_pos = Vector3::new(
        pos.x,
        entity.get_eye_y() - f64::from(hand_y_distance_from_eye),
        pos.z,
    );
    let direction = (target - pos).normalize();
    let velocity = Vector3::new(
        direction.x * throw_velocity.x,
        direction.y * throw_velocity.y,
        direction.z * throw_velocity.z,
    );
    let item_entity = ItemEntity::new_with_velocity(
        Entity::new(Arc::clone(&world), hand_pos, &EntityType::ITEM),
        item,
        velocity,
        ItemEntity::DEFAULT_PICKUP_DELAY,
    );
    world.spawn_entity(Arc::new(item_entity));
}

pub fn set_walk_and_look_target_memories(
    brain: &mut Brain,
    target: Arc<dyn PositionTracker>,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    brain.set(types::LOOK_TARGET, Arc::clone(&target));
    brain.set(
        types::WALK_TARGET,
        WalkTarget::new(target, speed_modifier, close_enough_dist),
    );
}

pub fn set_walk_and_look_target_memories_to_entity(
    brain: &mut Brain,
    target: Arc<dyn EntityBase>,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    set_walk_and_look_target_memories(
        brain,
        Arc::new(EntityTracker::new(target, true)),
        speed_modifier,
        close_enough_dist,
    );
}

pub fn set_walk_and_look_target_memories_to_block(
    brain: &mut Brain,
    target: pumpkin_util::math::position::BlockPos,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    set_walk_and_look_target_memories(
        brain,
        Arc::new(BlockPosTracker::new(target)),
        speed_modifier,
        close_enough_dist,
    );
}

#[must_use]
pub fn get_target_nearest_me(
    body: &dyn EntityBase,
    first: Arc<dyn EntityBase>,
    second: Arc<dyn EntityBase>,
) -> Arc<dyn EntityBase> {
    let pos = body.get_entity().pos.load();
    let first_dist = pos.squared_distance_to_vec(&first.get_entity().pos.load());
    let second_dist = pos.squared_distance_to_vec(&second.get_entity().pos.load());
    if first_dist < second_dist {
        first
    } else {
        second
    }
}

#[must_use]
pub fn get_nearest_target(
    body: &dyn EntityBase,
    first: Option<Arc<dyn EntityBase>>,
    second: Arc<dyn EntityBase>,
) -> Arc<dyn EntityBase> {
    match first {
        Some(first) => get_target_nearest_me(body, first, second),
        None => second,
    }
}

#[must_use]
pub fn get_living_entity_from_uuid_memory(
    brain: &Brain,
    world: &World,
    memory: MemoryModuleType<Uuid>,
) -> Option<Arc<dyn EntityBase>> {
    let uuid = *brain.get(memory)?;
    let entity = world.get_entity_by_uuid(uuid)?;
    entity.get_living_entity().is_some().then_some(entity)
}

#[must_use]
pub fn is_other_target_much_further_away_than_current_attack_target(
    brain: &Brain,
    body: &dyn EntityBase,
    other_target: &dyn EntityBase,
    how_much_further_away: f64,
) -> bool {
    let Some(current) = brain.get(types::ATTACK_TARGET) else {
        return false;
    };
    let pos = body.get_entity().pos.load();
    let dist_to_current = pos.squared_distance_to_vec(&current.get_entity().pos.load());
    let dist_to_other = pos.squared_distance_to_vec(&other_target.get_entity().pos.load());
    dist_to_other > dist_to_current + how_much_further_away * how_much_further_away
}

/// Vanilla `BehaviorUtils.isWithinAttackRange`.
#[must_use]
pub fn is_within_attack_range(
    mob: &dyn crate::entity::mob::Mob,
    target: &dyn EntityBase,
    projectile_attack_range_margin: i32,
) -> bool {
    mob.non_melee_weapon_range().map_or_else(
        || mob.get_mob_entity().is_in_attack_range(target),
        |range| {
            let max_allowed = f64::from(range - projectile_attack_range_margin);
            mob.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&target.get_entity().pos.load())
                < max_allowed * max_allowed
        },
    )
}

/// `World::dimension` is owned, so a `GlobalPos` resolves its static `Dimension` by name.
#[must_use]
pub fn global_pos_in(
    world: &World,
    pos: pumpkin_util::math::position::BlockPos,
) -> Option<super::super::memory::GlobalPos> {
    pumpkin_data::dimension::Dimension::from_name(world.dimension.minecraft_name)
        .map(|dimension| super::super::memory::GlobalPos::new(dimension, pos))
}

#[must_use]
pub fn is_breeding(brain: &Brain) -> bool {
    brain.has_memory_value(types::BREED_TARGET.id())
}

/// Vanilla `BehaviorUtils.getRandomSwimmablePos`: up to ten rerolls for a water-pathable spot.
#[must_use]
pub fn get_random_swimmable_pos(
    mob: &dyn crate::entity::mob::Mob,
    max_horizontal_distance: i32,
    max_vertical_distance: i32,
) -> Option<pumpkin_util::math::vector3::Vector3<f64>> {
    let world = mob.get_entity().world.load();
    let is_swimmable = |pos: pumpkin_util::math::vector3::Vector3<f64>| {
        let (block, state) =
            world.get_block_and_state(&pumpkin_util::math::position::BlockPos::floored_v(pos));
        world
            .block_registry
            .is_pathfindable(block, state, crate::block::PathComputationType::Water)
    };
    let mut target =
        default_random_pos::get_pos(mob, max_horizontal_distance, max_vertical_distance);
    let mut count = 0;
    while let Some(pos) = target {
        if is_swimmable(pos) || count >= 10 {
            break;
        }
        count += 1;
        target = default_random_pos::get_pos(mob, max_horizontal_distance, max_vertical_distance);
    }
    target
}
