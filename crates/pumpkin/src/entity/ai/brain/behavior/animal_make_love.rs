use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use rand::RngExt;

use crate::entity::EntityBase;
use crate::entity::passive::animal::spawn_child_from_breeding;

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, MemoryStatus, types};
use super::timed::Behavior;
use super::utils::{entity_is_visible, is_alive, set_walk_and_look_target_memories_to_entity};

const BREED_RANGE: f64 = 3.0;
const MIN_DURATION: i32 = 60;
const MAX_DURATION: i32 = 110;

pub struct AnimalMakeLove {
    conditions: [(MemoryModuleId, MemoryStatus); 5],
    partner_type: &'static EntityType,
    speed_modifier: f32,
    close_enough_distance: i32,
    spawn_child_at_time: i64,
}

impl AnimalMakeLove {
    #[must_use]
    pub const fn new(
        partner_type: &'static EntityType,
        speed_modifier: f32,
        close_enough_distance: i32,
    ) -> Self {
        Self {
            conditions: [
                (
                    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                    MemoryStatus::ValuePresent,
                ),
                (types::BREED_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::WALK_TARGET.id(), MemoryStatus::Registered),
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (types::IS_PANICKING.id(), MemoryStatus::ValueAbsent),
            ],
            partner_type,
            speed_modifier,
            close_enough_distance,
            spawn_child_at_time: 0,
        }
    }

    fn find_valid_breed_partner(&self, tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        let ctx = tick.visibility();
        ctx.brain
            .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?
            .find_closest(&ctx, |entity| {
                entity.get_entity().entity_type == self.partner_type
                    && entity.as_mob_entity().is_some()
                    && can_mate(tick.mob, entity.as_ref())
                    && !entity.is_panicking()
            })
    }

    fn breed_target(tick: &BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
        tick.brain.get(types::BREED_TARGET).map(Arc::clone)
    }

    /// Vanilla `BehaviorUtils.lockGazeAndWalkToEachOther`; the partner's half lands next tick.
    fn lock_gaze_and_walk_to_each_other(
        &self,
        tick: &mut BrainTick<'_>,
        partner: &Arc<dyn EntityBase>,
    ) {
        let speed_modifier = self.speed_modifier;
        let close_enough_distance = self.close_enough_distance;
        set_walk_and_look_target_memories_to_entity(
            tick.brain,
            Arc::clone(partner),
            speed_modifier,
            close_enough_distance,
        );
        post_to_partner(tick, partner, move |partner_tick, body| {
            set_walk_and_look_target_memories_to_entity(
                partner_tick.brain,
                body,
                speed_modifier,
                close_enough_distance,
            );
        });
    }
}

/// Vanilla `Animal.canMate`: another animal of the same type, both in love.
fn can_mate(body: &dyn EntityBase, partner: &dyn EntityBase) -> bool {
    partner.get_entity().entity_id != body.get_entity().entity_id
        && partner.get_entity().entity_type == body.get_entity().entity_type
        && body.is_in_love()
        && partner.is_in_love()
}

/// Queues `write` on the partner's brain, handing it this body; vanilla writes it inline.
fn post_to_partner(
    tick: &BrainTick<'_>,
    partner: &Arc<dyn EntityBase>,
    write: impl FnOnce(&mut BrainTick<'_>, Arc<dyn EntityBase>) + Send + 'static,
) {
    let Some(partner_mob) = partner.as_mob_entity() else {
        return;
    };
    let body_id = tick.mob.get_entity().entity_id;
    let Some(body) = tick.world.get_entity_by_id(body_id) else {
        return;
    };
    partner_mob.post_to_brain(Box::new(move |partner_tick| write(partner_tick, body)));
}

impl Behavior for AnimalMakeLove {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &self.conditions
    }

    fn min_duration(&self) -> i32 {
        MAX_DURATION
    }

    fn max_duration(&self) -> i32 {
        MAX_DURATION
    }

    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        tick.mob.is_in_love() && self.find_valid_breed_partner(tick).is_some()
    }

    fn start(&mut self, tick: &mut BrainTick<'_>) {
        let Some(partner) = self.find_valid_breed_partner(tick) else {
            return;
        };
        tick.brain.set(types::BREED_TARGET, Arc::clone(&partner));
        post_to_partner(tick, &partner, |partner_tick, body| {
            partner_tick.brain.set(types::BREED_TARGET, body);
        });
        self.lock_gaze_and_walk_to_each_other(tick, &partner);
        let duration = MIN_DURATION + tick.mob.get_random().random_range(0..50);
        self.spawn_child_at_time = tick.time + i64::from(duration);
    }

    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        let Some(partner) = Self::breed_target(tick) else {
            return false;
        };
        if partner.get_entity().entity_type != self.partner_type {
            return false;
        }
        is_alive(partner.as_ref())
            && can_mate(tick.mob, partner.as_ref())
            && entity_is_visible(&tick.visibility(), partner.as_ref())
            && tick.time <= self.spawn_child_at_time
            && !tick.mob.is_panicking()
            && !partner.is_panicking()
    }

    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(partner) = Self::breed_target(tick) else {
            return;
        };
        self.lock_gaze_and_walk_to_each_other(tick, &partner);
        let distance_squared = tick
            .mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&partner.get_entity().pos.load());
        if distance_squared < BREED_RANGE * BREED_RANGE && tick.time >= self.spawn_child_at_time {
            spawn_child_from_breeding(tick.mob, partner.as_ref());
            tick.brain.erase(types::BREED_TARGET.id());
            post_to_partner(tick, &partner, |partner_tick, _| {
                partner_tick.brain.erase(types::BREED_TARGET.id());
            });
        }
    }

    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::BREED_TARGET.id());
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::LOOK_TARGET.id());
        self.spawn_child_at_time = 0;
    }

    fn debug_name(&self) -> &'static str {
        "AnimalMakeLove"
    }
}
