use std::fmt;
use std::hash::{Hash, Hasher};

use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::position::BlockPos;

#[derive(Clone, Copy)]
pub struct GlobalPos {
    pub dimension: &'static Dimension,
    pub pos: BlockPos,
}

impl GlobalPos {
    #[must_use]
    pub const fn new(dimension: &'static Dimension, pos: BlockPos) -> Self {
        Self { dimension, pos }
    }

    /// Vanilla `GlobalPos.isCloseEnough`: same dimension and within a chessboard distance.
    #[must_use]
    pub fn is_close_enough(&self, dimension_name: &str, pos: &BlockPos, max_distance: i32) -> bool {
        let delta = self.pos.0 - pos.0;
        self.dimension.minecraft_name == dimension_name
            && delta.x.abs().max(delta.y.abs()).max(delta.z.abs()) <= max_distance
    }
}

impl PartialEq for GlobalPos {
    fn eq(&self, other: &Self) -> bool {
        self.dimension.id == other.dimension.id && self.pos == other.pos
    }
}

impl Eq for GlobalPos {}

impl Hash for GlobalPos {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.dimension.id.hash(state);
        self.pos.hash(state);
    }
}

impl fmt::Debug for GlobalPos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?}", self.dimension.minecraft_name, self.pos)
    }
}
