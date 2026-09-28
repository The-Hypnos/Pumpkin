use std::sync::Arc;

use pumpkin_data::tag::{self, Taggable};

use crate::entity::EntityBase;
use crate::entity::ageable::is_baby;
use crate::entity::mob::hoglin_ai::{
    REPELLENT_DETECTION_RANGE_HORIZONTAL, REPELLENT_DETECTION_RANGE_VERTICAL,
};
use crate::entity::mob::piglin_ai::{as_hoglin, as_piglin};

use super::super::BrainTick;
use super::super::memory::{MemoryModuleId, NearestVisibleLivingEntities, types};
use super::{Sensor, find_first_in_box_by_manhattan_distance};

const REQUIRES: &[MemoryModuleId] = &[
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
    types::NEAREST_REPELLENT.id(),
    types::NEAREST_VISIBLE_ADULT_PIGLIN.id(),
    types::NEAREST_VISIBLE_ADULT_HOGLINS.id(),
    types::VISIBLE_ADULT_PIGLIN_COUNT.id(),
    types::VISIBLE_ADULT_HOGLIN_COUNT.id(),
];

pub struct HoglinSpecificSensor;

impl Sensor for HoglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let world = Arc::clone(tick.world);
        let repellent = find_first_in_box_by_manhattan_distance(
            tick.mob.get_entity().block_pos.load(),
            REPELLENT_DETECTION_RANGE_HORIZONTAL,
            REPELLENT_DETECTION_RANGE_VERTICAL,
            |pos| {
                world
                    .get_block(pos)
                    .has_tag(&tag::Block::MINECRAFT_HOGLIN_REPELLENTS)
            },
        );
        tick.brain.set_optional(types::NEAREST_REPELLENT, repellent);

        let mut adult_piglin: Option<Arc<dyn EntityBase>> = None;
        let mut adult_piglin_count = 0;
        let mut adult_hoglins: Vec<Arc<dyn EntityBase>> = Vec::new();
        {
            let ctx = tick.visibility();
            let empty = NearestVisibleLivingEntities::empty();
            let visible = ctx
                .brain
                .get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                .unwrap_or(&empty);
            for entity in visible.find_all(&ctx, |entity| {
                !is_baby(entity.as_ref())
                    && (as_piglin(entity.as_ref()).is_some()
                        || as_hoglin(entity.as_ref()).is_some())
            }) {
                if as_piglin(entity.as_ref()).is_some() {
                    adult_piglin_count += 1;
                    if adult_piglin.is_none() {
                        adult_piglin = Some(Arc::clone(entity));
                    }
                }
                if as_hoglin(entity.as_ref()).is_some() {
                    adult_hoglins.push(Arc::clone(entity));
                }
            }
        }

        let adult_hoglin_count = i32::try_from(adult_hoglins.len()).unwrap_or(i32::MAX);
        tick.brain
            .set_optional(types::NEAREST_VISIBLE_ADULT_PIGLIN, adult_piglin);
        tick.brain
            .set(types::NEAREST_VISIBLE_ADULT_HOGLINS, adult_hoglins);
        tick.brain
            .set(types::VISIBLE_ADULT_PIGLIN_COUNT, adult_piglin_count);
        tick.brain
            .set(types::VISIBLE_ADULT_HOGLIN_COUNT, adult_hoglin_count);
    }
}
