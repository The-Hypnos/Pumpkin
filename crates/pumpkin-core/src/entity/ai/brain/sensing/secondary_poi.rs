use pumpkin_util::math::position::BlockPos;

use crate::entity::passive::villager::VillagerEntity;

use super::super::BrainTick;
use super::super::behavior::utils::global_pos_in;
use super::super::memory::{MemoryModuleId, types};
use super::Sensor;

pub const SECONDARY_POI_SCAN_RATE: i32 = 40;
const HORIZONTAL_SEARCH: i32 = 4;
const VERTICAL_SEARCH: i32 = 2;

const REQUIRES: &[MemoryModuleId] = &[types::SECONDARY_JOB_SITE.id()];

/// Vanilla `SecondaryPoiSensor`: blocks next to a villager its profession also works at.
pub struct SecondaryPoiSensor;

impl Sensor for SecondaryPoiSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        REQUIRES
    }

    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(villager) = tick.mob.cast_any().downcast_ref::<VillagerEntity>() else {
            return;
        };
        let secondary = villager.profession().secondary_poi();
        let center = tick.mob.get_entity().block_pos.load();
        let min = BlockPos::new(
            center.0.x - HORIZONTAL_SEARCH,
            center.0.y - VERTICAL_SEARCH,
            center.0.z - HORIZONTAL_SEARCH,
        );
        let max = BlockPos::new(
            center.0.x + HORIZONTAL_SEARCH,
            center.0.y + VERTICAL_SEARCH,
            center.0.z + HORIZONTAL_SEARCH,
        );
        // Most professions have no secondary POI, and the palettes rule out most areas, so the
        // block walk below rarely runs.
        let mut job_sites = Vec::new();
        if !secondary.is_empty()
            && tick
                .world
                .may_contain_block_state(min, max, |state| secondary.contains(&state.to_block()))
        {
            for x in -HORIZONTAL_SEARCH..=HORIZONTAL_SEARCH {
                for y in -VERTICAL_SEARCH..=VERTICAL_SEARCH {
                    for z in -HORIZONTAL_SEARCH..=HORIZONTAL_SEARCH {
                        let pos = BlockPos::new(center.0.x + x, center.0.y + y, center.0.z + z);
                        if secondary.contains(&tick.world.get_block(&pos))
                            && let Some(global) = global_pos_in(tick.world, pos)
                        {
                            job_sites.push(global);
                        }
                    }
                }
            }
        }
        if job_sites.is_empty() {
            tick.brain.erase(types::SECONDARY_JOB_SITE.id());
        } else {
            tick.brain.set(types::SECONDARY_JOB_SITE, job_sites);
        }
    }
}
