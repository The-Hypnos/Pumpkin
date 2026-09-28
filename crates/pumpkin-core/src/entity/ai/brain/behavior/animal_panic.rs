use pumpkin_data::tag::{self, Tag, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::brain::sensing::find_first_in_box_by_manhattan_distance;
use crate::entity::ai::util::goal_utils::is_water_state;
use crate::entity::ai::util::land_random_pos;
use crate::entity::mob::Mob;
use crate::world::World;

use super::super::BrainTick;
use super::super::memory::walk_target::WalkTarget;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;

const PANIC_MIN_DURATION: i32 = 100;
const PANIC_MAX_DURATION: i32 = 120;
const PANIC_DISTANCE_HORIZONTAL: i32 = 5;
const PANIC_DISTANCE_VERTICAL: i32 = 4;
const WATER_SEARCH_HORIZONTAL: i32 = 5;
const WATER_SEARCH_VERTICAL: i32 = 1;

pub type PanicCauses = fn(&dyn Mob) -> &'static Tag;
pub type PanicPos = fn(&dyn Mob) -> Option<Vector3<f64>>;

/// Vanilla `AnimalPanic`: flees after taking a panic-causing hit, towards water when burning.
pub struct AnimalPanic {
    conditions: [(MemoryModuleId, MemoryStatus); 2],
    speed_multiplier: f32,
    panic_causing_damage_types: PanicCauses,
    position_getter: PanicPos,
}

impl AnimalPanic {
    #[must_use]
    pub fn new(speed_multiplier: f32) -> Self {
        Self::with(
            speed_multiplier,
            |_| &tag::DamageType::MINECRAFT_PANIC_CAUSES,
            |mob| land_random_pos::get_pos(mob, PANIC_DISTANCE_HORIZONTAL, PANIC_DISTANCE_VERTICAL),
        )
    }

    #[must_use]
    pub const fn with(
        speed_multiplier: f32,
        panic_causing_damage_types: PanicCauses,
        position_getter: PanicPos,
    ) -> Self {
        Self {
            conditions: [
                (types::IS_PANICKING.id(), MemoryStatus::Registered),
                (types::HURT_BY.id(), MemoryStatus::Registered),
            ],
            speed_multiplier,
            panic_causing_damage_types,
            position_getter,
        }
    }

    fn get_panic_pos(&self, mob: &dyn Mob) -> Option<Vector3<f64>> {
        if mob.get_entity().is_on_fire()
            && let Some(water) = look_for_water(&mob.get_entity().world.load(), mob)
        {
            return Some(Vector3::new(
                f64::from(water.0.x) + 0.5,
                f64::from(water.0.y),
                f64::from(water.0.z) + 0.5,
            ));
        }
        (self.position_getter)(mob)
    }
}

fn look_for_water(world: &World, mob: &dyn Mob) -> Option<BlockPos> {
    let mob_pos = mob.get_entity().block_pos.load();
    if world
        .get_block_state(&mob_pos)
        .get_block_collision_shapes_at(&mob_pos)
        .next()
        .is_some()
    {
        return None;
    }
    if !world.may_contain_block_state(
        mob_pos.add(
            -WATER_SEARCH_HORIZONTAL,
            -WATER_SEARCH_VERTICAL,
            -WATER_SEARCH_HORIZONTAL,
        ),
        mob_pos.add(
            WATER_SEARCH_HORIZONTAL,
            WATER_SEARCH_VERTICAL,
            WATER_SEARCH_HORIZONTAL,
        ),
        is_water_state,
    ) {
        return None;
    }
    let is_water_at = |pos: &BlockPos| {
        world
            .get_block_state_id_if_loaded(pos)
            .is_some_and(is_water_state)
    };
    let two_wide = mob.get_entity().entity_dimension.load().width.ceil() as i32 == 2;
    find_first_in_box_by_manhattan_distance(
        mob_pos,
        WATER_SEARCH_HORIZONTAL,
        WATER_SEARCH_VERTICAL,
        |pos| {
            is_water_at(pos)
                && (!two_wide
                    || (is_water_at(&pos.south())
                        && is_water_at(&pos.east())
                        && is_water_at(&pos.south().east())))
        },
    )
}

impl Behavior for AnimalPanic {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn min_duration(&self) -> i32 {
        PANIC_MIN_DURATION
    }

    fn max_duration(&self) -> i32 {
        PANIC_MAX_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        let causes = (self.panic_causing_damage_types)(tick.mob);
        tick.brain
            .get(types::HURT_BY)
            .is_some_and(|hurt_by| hurt_by.damage_type.has_tag(causes))
            || tick.brain.has_memory_value(types::IS_PANICKING.id())
    }

    fn can_still_use(&mut self, _tick: &BrainTick<'_>) -> bool {
        true
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.set(types::IS_PANICKING, true);
        tick.brain.erase(types::WALK_TARGET.id());
        tick.mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let navigation_done = tick
            .mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_done();
        if navigation_done && let Some(pos) = self.get_panic_pos(tick.mob) {
            tick.brain.set(
                types::WALK_TARGET,
                WalkTarget::from_vec(pos, self.speed_multiplier, 0),
            );
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::IS_PANICKING.id());
    }

    fn debug_name(&self) -> &'static str {
        "AnimalPanic"
    }
}
