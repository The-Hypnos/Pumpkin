//! A per-tick index of entities by chunk, so area queries from AI touch only nearby entities
//! instead of every entity in the world.

use std::sync::Arc;

use pumpkin_util::math::boundingbox::BoundingBox;
use rustc_hash::FxHashMap;

use crate::entity::EntityBase;

/// How far outside a queried box an entity's position may lie and still be found. Vanilla's
/// `EntitySectionStorage.getEntities` widens the section range by the same amount.
const QUERY_MARGIN: f64 = 2.0;

#[derive(Default)]
pub struct EntityGrid {
    cells: FxHashMap<(i32, i32), Vec<Arc<dyn EntityBase>>>,
}

impl EntityGrid {
    /// Buckets every entity by the chunk its position is in.
    pub fn build<'a>(entities: impl Iterator<Item = &'a Arc<dyn EntityBase>>) -> Self {
        let mut cells: FxHashMap<(i32, i32), Vec<Arc<dyn EntityBase>>> = FxHashMap::default();
        for entity in entities {
            let pos = entity.get_entity().pos.load();
            let key = ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4);
            cells.entry(key).or_default().push(Arc::clone(entity));
        }
        Self { cells }
    }

    /// Every indexed entity whose current bounding box meets `aabb`, players included.
    #[must_use]
    pub fn collect_in_box(&self, aabb: &BoundingBox) -> Vec<Arc<dyn EntityBase>> {
        let mut found = Vec::new();
        self.for_each_in_box(aabb, |entity| found.push(Arc::clone(entity)));
        found
    }

    /// Calls `visit` for every indexed entity whose current bounding box meets `aabb`.
    pub fn for_each_in_box(&self, aabb: &BoundingBox, mut visit: impl FnMut(&Arc<dyn EntityBase>)) {
        let min_x = ((aabb.min.x - QUERY_MARGIN).floor() as i32) >> 4;
        let max_x = ((aabb.max.x + QUERY_MARGIN).floor() as i32) >> 4;
        let min_z = ((aabb.min.z - QUERY_MARGIN).floor() as i32) >> 4;
        let max_z = ((aabb.max.z + QUERY_MARGIN).floor() as i32) >> 4;
        for chunk_z in min_z..=max_z {
            for chunk_x in min_x..=max_x {
                let Some(cell) = self.cells.get(&(chunk_x, chunk_z)) else {
                    continue;
                };
                for entity in cell {
                    if entity.get_entity().bounding_box.load().intersects(aabb) {
                        visit(entity);
                    }
                }
            }
        }
    }
}
