use pumpkin_util::{
    HeightMap,
    math::{block_box::BlockBox, vector3::Axis},
};

use crate::{
    ProtoChunk,
    generation::{
        proto_chunk::GenerationCache,
        structure::{piece::StructurePieceType, structures::StructurePiece},
    },
};

pub const DEFAULT_H_POS: i32 = -1;

pub struct ShiftableStructurePiece {
    pub piece: StructurePiece,
    h_pos: i32,
    width: i32,
    height: i32,
}

impl ShiftableStructurePiece {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        r#type: StructurePieceType,
        x: i32,
        y: i32,
        z: i32,
        width: i32,
        height: i32,
        depth: i32,
        axis: Axis,
    ) -> Self {
        Self {
            piece: StructurePiece::new(
                r#type,
                BlockBox::create_box(x, y, z, axis, width, height, depth),
                0,
            ),
            h_pos: DEFAULT_H_POS,
            width,
            height,
        }
    }

    pub fn update_average_ground_height(
        &mut self,
        chunk: &ProtoChunk,
        chunk_box: &BlockBox,
        offset: i32,
    ) -> bool {
        if self.h_pos >= 0 {
            return true;
        }
        let bounding_box = self.piece.bounding_box;
        let Some(height) = average_ground_height(chunk, &bounding_box, chunk_box) else {
            return false;
        };
        self.h_pos = height;
        self.piece
            .bounding_box
            .move_pos(0, height - bounding_box.min.y + offset, 0);
        true
    }
}

#[must_use]
pub fn average_ground_height(
    chunk: &ProtoChunk,
    bounding_box: &BlockBox,
    chunk_box: &BlockBox,
) -> Option<i32> {
    let mut total = 0;
    let mut count = 0;
    for z in bounding_box.min.z..=bounding_box.max.z {
        for x in bounding_box.min.x..=bounding_box.max.x {
            if chunk_box.contains(x, 64, z) {
                total += chunk.get_top_y(&HeightMap::MotionBlockingNoLeaves, x, z);
                count += 1;
            }
        }
    }
    (count > 0).then(|| total / count)
}

#[must_use]
pub fn lowest_ground_height(region: &dyn GenerationCache, bounding_box: &BlockBox) -> i32 {
    let mut lowest = i32::MAX;
    for z in bounding_box.min.z..=bounding_box.max.z {
        for x in bounding_box.min.x..=bounding_box.max.x {
            lowest = lowest.min(region.get_top_y(&HeightMap::MotionBlockingNoLeaves, x, z));
        }
    }
    lowest
}
