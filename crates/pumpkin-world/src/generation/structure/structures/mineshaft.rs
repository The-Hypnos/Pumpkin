use std::sync::{Arc, Mutex};

use pumpkin_data::{
    Block, BlockState,
    block_properties::{
        HorizontalFacing, OakFenceLikeProperties, RailLikeProperties, RailShape,
        WallTorchLikeProperties,
    },
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::{
    BlockDirection,
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::{
    ProtoChunk,
    generation::structure::{
        piece::StructurePieceType,
        structures::{
            StructureGenerator, StructureGeneratorContext, StructurePiece, StructurePieceBase,
            StructurePiecesCollector, StructurePosition, WorldPortalExt,
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MineshaftType {
    Normal,
    Mesa,
}

impl MineshaftType {
    #[must_use]
    pub const fn wood(self) -> &'static BlockState {
        match self {
            Self::Normal => Block::OAK_LOG.default_state,
            Self::Mesa => Block::DARK_OAK_LOG.default_state,
        }
    }

    #[must_use]
    pub const fn planks(self) -> &'static BlockState {
        match self {
            Self::Normal => Block::OAK_PLANKS.default_state,
            Self::Mesa => Block::DARK_OAK_PLANKS.default_state,
        }
    }

    #[must_use]
    pub const fn fence(self) -> &'static BlockState {
        match self {
            Self::Normal => Block::OAK_FENCE.default_state,
            Self::Mesa => Block::DARK_OAK_FENCE.default_state,
        }
    }
}

fn rail(shape: RailShape) -> &'static BlockState {
    let mut props = RailLikeProperties::default(&Block::RAIL);
    props.shape = shape;
    BlockState::from_id(props.to_state_id(&Block::RAIL))
}

fn wall_torch(facing: HorizontalFacing) -> &'static BlockState {
    let mut props = WallTorchLikeProperties::default(&Block::WALL_TORCH);
    props.facing = facing;
    BlockState::from_id(props.to_state_id(&Block::WALL_TORCH))
}

fn is_replaceable_by_structures(state: &BlockState) -> bool {
    let block = Block::from_state_id(state.id);
    state.is_air()
        || state.is_liquid()
        || block == &Block::GLOW_LICHEN
        || block == &Block::SEAGRASS
        || block == &Block::TALL_SEAGRASS
}

fn can_hang_chain_below(state: &BlockState) -> bool {
    let block = Block::from_state_id(state.id);
    let falling = [
        &Block::SAND,
        &Block::RED_SAND,
        &Block::GRAVEL,
        &Block::DRAGON_EGG,
        &Block::ANVIL,
        &Block::CHIPPED_ANVIL,
        &Block::DAMAGED_ANVIL,
    ]
    .contains(&block)
        || block.name.ends_with("_concrete_powder");
    state.is_center_solid(pumpkin_data::BlockDirection::Down) && !falling
}

struct ShaftPlacer<'a> {
    piece: &'a StructurePiece,
    shaft_type: MineshaftType,
    chunk_box: &'a BlockBox,
}

impl ShaftPlacer<'_> {
    fn can_be_replaced(&self, chunk: &ProtoChunk, x: i32, y: i32, z: i32) -> bool {
        let block =
            Block::from_state_id(self.piece.get_block_at(chunk, x, y, z, self.chunk_box).id);
        block != Block::from_state_id(self.shaft_type.planks().id)
            && block != Block::from_state_id(self.shaft_type.wood().id)
            && block != Block::from_state_id(self.shaft_type.fence().id)
            && block != &Block::IRON_CHAIN
    }

    fn place_block(&self, chunk: &mut ProtoChunk, state: &BlockState, x: i32, y: i32, z: i32) {
        if self.can_be_replaced(chunk, x, y, z) {
            self.piece.add_block(chunk, state, x, y, z, self.chunk_box);
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn generate_box(
        &self,
        chunk: &mut ProtoChunk,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        edge: &BlockState,
        fill: &BlockState,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    let is_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place_block(chunk, if is_edge { edge } else { fill }, x, y, z);
                }
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn generate_maybe_box(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        probability: f32,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        edge: &BlockState,
        fill: &BlockState,
        has_to_be_inside: bool,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if random.next_f32() > probability
                        || (has_to_be_inside
                            && !self
                                .piece
                                .is_under_sea_level(chunk, x, y, z, self.chunk_box))
                    {
                        continue;
                    }
                    let is_edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place_block(chunk, if is_edge { edge } else { fill }, x, y, z);
                }
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn maybe_generate_block(
        &self,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        probability: f32,
        x: i32,
        y: i32,
        z: i32,
        state: &BlockState,
    ) {
        if random.next_f32() < probability {
            self.place_block(chunk, state, x, y, z);
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn generate_upper_half_sphere(
        &self,
        chunk: &mut ProtoChunk,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        fill: &BlockState,
    ) {
        let diag_x = (x1 - x0 + 1) as f32;
        let diag_y = (y1 - y0 + 1) as f32;
        let diag_z = (z1 - z0 + 1) as f32;
        let cx = x0 as f32 + diag_x / 2.0;
        let cz = z0 as f32 + diag_z / 2.0;
        for y in y0..=y1 {
            let ny = (y - y0) as f32 / diag_y;
            for x in x0..=x1 {
                let nx = (x as f32 - cx) / (diag_x * 0.5);
                for z in z0..=z1 {
                    let nz = (z as f32 - cz) / (diag_z * 0.5);
                    if nx * nx + ny * ny + nz * nz <= 1.05 {
                        self.place_block(chunk, fill, x, y, z);
                    }
                }
            }
        }
    }

    fn is_in_invalid_location(&self, chunk: &ProtoChunk) -> bool {
        let bb = self.piece.bounding_box;
        let cb = self.chunk_box;
        let x0 = (bb.min.x - 1).max(cb.min.x);
        let y0 = (bb.min.y - 1).max(cb.min.y);
        let z0 = (bb.min.z - 1).max(cb.min.z);
        let x1 = (bb.max.x + 1).min(cb.max.x);
        let y1 = (bb.max.y + 1).min(cb.max.y);
        let z1 = (bb.max.z + 1).min(cb.max.z);

        let biome = chunk.get_terrain_gen_biome_id(
            i32::midpoint(x0, x1),
            i32::midpoint(y0, y1),
            i32::midpoint(z0, z1),
        );
        if pumpkin_data::tag::get_tag_ids(
            pumpkin_data::tag::RegistryKey::WorldgenBiome,
            "minecraft:mineshaft_blocking",
        )
        .is_some_and(|ids| ids.contains(&u16::from(biome)))
        {
            return true;
        }

        let is_liquid = |x: i32, y: i32, z: i32| {
            chunk
                .get_block_state(&Vector3::new(x, y, z))
                .to_state()
                .is_liquid()
        };
        for x in x0..=x1 {
            for z in z0..=z1 {
                if is_liquid(x, y0, z) || is_liquid(x, y1, z) {
                    return true;
                }
            }
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                if is_liquid(x, y, z0) || is_liquid(x, y, z1) {
                    return true;
                }
            }
        }
        for z in z0..=z1 {
            for y in y0..=y1 {
                if is_liquid(x0, y, z) || is_liquid(x1, y, z) {
                    return true;
                }
            }
        }
        false
    }

    fn is_supporting_box(&self, chunk: &ProtoChunk, x0: i32, x1: i32, y1: i32, z0: i32) -> bool {
        (x0..=x1).all(|x| {
            !self
                .piece
                .get_block_at(chunk, x, y1 + 1, z0, self.chunk_box)
                .is_air()
        })
    }

    fn set_planks_block(&self, chunk: &mut ProtoChunk, x: i32, y: i32, z: i32) {
        if self
            .piece
            .is_under_sea_level(chunk, x, y, z, self.chunk_box)
        {
            let pos = self.piece.offset_pos(x, y, z);
            if !chunk
                .get_block_state(&pos)
                .to_state()
                .is_side_solid(pumpkin_data::BlockDirection::Up)
            {
                chunk.set_block_state(pos.x, pos.y, pos.z, self.shaft_type.planks());
            }
        }
    }

    fn is_interior(&self, chunk: &ProtoChunk, x: i32, y: i32, z: i32) -> bool {
        self.piece
            .is_under_sea_level(chunk, x, y, z, self.chunk_box)
    }

    fn get_block(&self, chunk: &ProtoChunk, x: i32, y: i32, z: i32) -> &'static BlockState {
        BlockState::from_id(self.piece.get_block_at(chunk, x, y, z, self.chunk_box).id)
    }
}

pub struct MineshaftGenerator {
    pub is_mesa: bool,
}

impl StructureGenerator for MineshaftGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        context.random.next_f64();
        let start_x = context.chunk_x << 4;
        let start_z = context.chunk_z << 4;
        let room_x = start_x + 2;
        let room_z = start_z + 2;

        let shaft_type = if self.is_mesa {
            MineshaftType::Mesa
        } else {
            MineshaftType::Normal
        };

        let start_room = MineShaftRoom::new(0, &mut context.random, room_x, room_z, shaft_type);
        let entrances = start_room.child_entrance_boxes.clone();
        let start_piece_box = start_room.piece.bounding_box;

        let mut collector = StructurePiecesCollector::default();
        collector.add_piece(Box::new(start_room));

        MineShaftRoom::add_room_children(
            &start_piece_box,
            shaft_type,
            &mut collector,
            &mut context.random,
            &entrances,
        );

        let dy = if self.is_mesa {
            let center = collector.get_bounding_box().get_center();

            let surface_height = context
                .height_sampler
                .as_deref_mut()
                .map_or(context.sea_level, |s| s.estimate_height(center.x, center.z));

            let target_y = if surface_height <= context.sea_level {
                context.sea_level
            } else {
                let range = surface_height - context.sea_level + 1;
                context.sea_level + context.random.next_bounded_i32(range)
            };

            let dy = target_y - center.y;
            collector.shift(dy);
            dy
        } else {
            collector.shift_into(context.sea_level, context.min_y, &mut context.random, 10)
        };

        Some(StructurePosition {
            start_pos: BlockPos::new(start_x + 8, 50 + dy, start_z),
            collector: Arc::new(collector.into()),
        })
    }
}

enum GeneratedPiece {
    Corridor(MineShaftCorridor),
    Crossing(MineShaftCrossing),
    Stairs(MineShaftStairs),
}

impl GeneratedPiece {
    const fn bounding_box(&self) -> BlockBox {
        match self {
            Self::Corridor(c) => c.piece.bounding_box,
            Self::Crossing(cr) => cr.piece.bounding_box,
            Self::Stairs(s) => s.piece.bounding_box,
        }
    }
}

#[expect(clippy::too_many_arguments)]
fn create_random_shaft_piece(
    collector: &StructurePiecesCollector,
    random: &mut RandomGenerator,
    foot_x: i32,
    foot_y: i32,
    foot_z: i32,
    direction: BlockDirection,
    gen_depth: u32,
    shaft_type: MineshaftType,
) -> Option<GeneratedPiece> {
    let random_selection = random.next_bounded_i32(100);
    if random_selection >= 80 {
        if let Some(crossing_box) =
            MineShaftCrossing::find_crossing(collector, random, foot_x, foot_y, foot_z, direction)
        {
            return Some(GeneratedPiece::Crossing(MineShaftCrossing::new(
                gen_depth,
                crossing_box,
                Some(direction),
                shaft_type,
            )));
        }
    } else if random_selection >= 70 {
        if let Some(stairs_box) =
            MineShaftStairs::find_stairs(collector, random, foot_x, foot_y, foot_z, direction)
        {
            return Some(GeneratedPiece::Stairs(MineShaftStairs::new(
                gen_depth, stairs_box, direction, shaft_type,
            )));
        }
    } else if let Some(corridor_box) =
        MineShaftCorridor::find_corridor_size(collector, random, foot_x, foot_y, foot_z, direction)
    {
        return Some(GeneratedPiece::Corridor(MineShaftCorridor::new(
            gen_depth,
            random,
            corridor_box,
            direction,
            shaft_type,
        )));
    }
    None
}

#[expect(clippy::too_many_arguments)]
fn generate_and_add_piece(
    start_piece_box: &BlockBox,
    collector: &mut StructurePiecesCollector,
    random: &mut RandomGenerator,
    foot_x: i32,
    foot_y: i32,
    foot_z: i32,
    direction: BlockDirection,
    depth: u32,
    shaft_type: MineshaftType,
) -> Option<BlockBox> {
    if depth > 8 {
        return None;
    }

    if (foot_x - start_piece_box.min.x).abs() <= 80
        && (foot_z - start_piece_box.min.z).abs() <= 80
        && let Some(new_piece) = create_random_shaft_piece(
            collector,
            random,
            foot_x,
            foot_y,
            foot_z,
            direction,
            depth + 1,
            shaft_type,
        )
    {
        let piece_box = new_piece.bounding_box();
        match new_piece {
            GeneratedPiece::Corridor(corridor) => {
                let bb = corridor.piece.bounding_box;
                let orientation = corridor.piece.facing.unwrap_or(BlockDirection::North);
                collector.add_piece(Box::new(corridor));
                MineShaftCorridor::add_corridor_children(
                    &bb,
                    depth + 1,
                    orientation,
                    start_piece_box,
                    shaft_type,
                    collector,
                    random,
                );
            }
            GeneratedPiece::Crossing(crossing) => {
                let bb = crossing.piece.bounding_box;
                let dir = crossing.direction;
                let is_two_floored = crossing.is_two_floored;
                collector.add_piece(Box::new(crossing));
                MineShaftCrossing::add_crossing_children(
                    &bb,
                    depth + 1,
                    dir,
                    is_two_floored,
                    start_piece_box,
                    shaft_type,
                    collector,
                    random,
                );
            }
            GeneratedPiece::Stairs(stairs) => {
                let bb = stairs.piece.bounding_box;
                let facing = stairs.piece.facing;
                collector.add_piece(Box::new(stairs));
                MineShaftStairs::add_stairs_children(
                    &bb,
                    depth + 1,
                    facing,
                    start_piece_box,
                    shaft_type,
                    collector,
                    random,
                );
            }
        }
        Some(piece_box)
    } else {
        None
    }
}

pub struct MineShaftRoom {
    pub piece: StructurePiece,
    pub shaft_type: MineshaftType,
    pub child_entrance_boxes: Arc<Mutex<Vec<BlockBox>>>,
}

impl MineShaftRoom {
    #[must_use]
    pub fn new(
        gen_depth: u32,
        random: &mut RandomGenerator,
        west: i32,
        north: i32,
        shaft_type: MineshaftType,
    ) -> Self {
        let max_x = west + 7 + random.next_bounded_i32(6);
        let max_y = 54 + random.next_bounded_i32(6);
        let max_z = north + 7 + random.next_bounded_i32(6);
        let bounding_box = BlockBox::new(west, 50, north, max_x, max_y, max_z);
        Self {
            piece: StructurePiece::new(StructurePieceType::MineshaftRoom, bounding_box, gen_depth),
            shaft_type,
            child_entrance_boxes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[expect(clippy::too_many_lines)]
    fn add_room_children(
        start_piece_box: &BlockBox,
        shaft_type: MineshaftType,
        collector: &mut StructurePiecesCollector,
        random: &mut RandomGenerator,
        child_entrance_boxes: &Arc<Mutex<Vec<BlockBox>>>,
    ) {
        let depth = 0;
        let y_span = start_piece_box.max.y - start_piece_box.min.y + 1;
        let mut height_space = y_span - 4;
        if height_space <= 0 {
            height_space = 1;
        }

        let x_span = start_piece_box.max.x - start_piece_box.min.x + 1;
        let z_span = start_piece_box.max.z - start_piece_box.min.z + 1;

        let mut pos = 0;
        while pos < x_span {
            pos += random.next_bounded_i32(x_span);
            if pos + 3 > x_span {
                break;
            }
            let child_x = start_piece_box.min.x + pos;
            let child_y = start_piece_box.min.y + random.next_bounded_i32(height_space) + 1;
            let child_z = start_piece_box.min.z - 1;

            if let Some(child_box) = generate_and_add_piece(
                start_piece_box,
                collector,
                random,
                child_x,
                child_y,
                child_z,
                BlockDirection::North,
                depth,
                shaft_type,
            ) {
                child_entrance_boxes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(BlockBox::new(
                        child_box.min.x,
                        child_box.min.y,
                        start_piece_box.min.z,
                        child_box.max.x,
                        child_box.max.y,
                        start_piece_box.min.z + 1,
                    ));
            }
            pos += 4;
        }

        pos = 0;
        while pos < x_span {
            pos += random.next_bounded_i32(x_span);
            if pos + 3 > x_span {
                break;
            }
            let child_x = start_piece_box.min.x + pos;
            let child_y = start_piece_box.min.y + random.next_bounded_i32(height_space) + 1;
            let child_z = start_piece_box.max.z + 1;

            if let Some(child_box) = generate_and_add_piece(
                start_piece_box,
                collector,
                random,
                child_x,
                child_y,
                child_z,
                BlockDirection::South,
                depth,
                shaft_type,
            ) {
                child_entrance_boxes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(BlockBox::new(
                        child_box.min.x,
                        child_box.min.y,
                        start_piece_box.max.z - 1,
                        child_box.max.x,
                        child_box.max.y,
                        start_piece_box.max.z,
                    ));
            }
            pos += 4;
        }

        pos = 0;
        while pos < z_span {
            pos += random.next_bounded_i32(z_span);
            if pos + 3 > z_span {
                break;
            }
            let child_x = start_piece_box.min.x - 1;
            let child_y = start_piece_box.min.y + random.next_bounded_i32(height_space) + 1;
            let child_z = start_piece_box.min.z + pos;

            if let Some(child_box) = generate_and_add_piece(
                start_piece_box,
                collector,
                random,
                child_x,
                child_y,
                child_z,
                BlockDirection::West,
                depth,
                shaft_type,
            ) {
                child_entrance_boxes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(BlockBox::new(
                        start_piece_box.min.x,
                        child_box.min.y,
                        child_box.min.z,
                        start_piece_box.min.x + 1,
                        child_box.max.y,
                        child_box.max.z,
                    ));
            }
            pos += 4;
        }

        pos = 0;
        while pos < z_span {
            pos += random.next_bounded_i32(z_span);
            if pos + 3 > z_span {
                break;
            }
            let child_x = start_piece_box.max.x + 1;
            let child_y = start_piece_box.min.y + random.next_bounded_i32(height_space) + 1;
            let child_z = start_piece_box.min.z + pos;

            if let Some(child_box) = generate_and_add_piece(
                start_piece_box,
                collector,
                random,
                child_x,
                child_y,
                child_z,
                BlockDirection::East,
                depth,
                shaft_type,
            ) {
                child_entrance_boxes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(BlockBox::new(
                        start_piece_box.max.x - 1,
                        child_box.min.y,
                        child_box.min.z,
                        start_piece_box.max.x,
                        child_box.max.y,
                        child_box.max.z,
                    ));
            }
            pos += 4;
        }
    }
}

impl StructurePieceBase for MineShaftRoom {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }

    fn translate(&mut self, x: i32, y: i32, z: i32) {
        self.piece.translate(x, y, z);
        for bb in &mut *self
            .child_entrance_boxes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            bb.move_pos(x, y, z);
        }
    }

    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn WorldPortalExt,
        _random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let placer = ShaftPlacer {
            piece: &self.piece,
            shaft_type: self.shaft_type,
            chunk_box,
        };
        if placer.is_in_invalid_location(chunk) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        let bb = self.piece.bounding_box;

        placer.generate_box(
            chunk,
            bb.min.x,
            bb.min.y + 1,
            bb.min.z,
            bb.max.x,
            (bb.min.y + 3).min(bb.max.y),
            bb.max.z,
            air,
            air,
        );

        for entrance in &*self
            .child_entrance_boxes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            placer.generate_box(
                chunk,
                entrance.min.x,
                entrance.max.y - 2,
                entrance.min.z,
                entrance.max.x,
                entrance.max.y,
                entrance.max.z,
                air,
                air,
            );
        }

        placer.generate_upper_half_sphere(
            chunk,
            bb.min.x,
            bb.min.y + 4,
            bb.min.z,
            bb.max.x,
            bb.max.y,
            bb.max.z,
            air,
        );
    }
}

pub struct MineShaftCorridor {
    pub piece: StructurePiece,
    pub shaft_type: MineshaftType,
    pub has_rails: bool,
    pub spider_corridor: bool,
    pub has_placed_spider: bool,
    pub num_sections: i32,
}

impl MineShaftCorridor {
    #[must_use]
    pub fn new(
        gen_depth: u32,
        random: &mut RandomGenerator,
        bounding_box: BlockBox,
        direction: BlockDirection,
        shaft_type: MineshaftType,
    ) -> Self {
        let has_rails = random.next_bounded_i32(3) == 0;
        let spider_corridor = !has_rails && random.next_bounded_i32(23) == 0;
        let axis = direction.get_axis();
        let num_sections = if axis == pumpkin_util::math::vector3::Axis::Z {
            (bounding_box.max.z - bounding_box.min.z + 1) / 5
        } else {
            (bounding_box.max.x - bounding_box.min.x + 1) / 5
        };

        let mut piece = StructurePiece::new(
            StructurePieceType::MineshaftCorridor,
            bounding_box,
            gen_depth,
        );
        piece.set_facing(Some(direction));

        Self {
            piece,
            shaft_type,
            has_rails,
            spider_corridor,
            has_placed_spider: false,
            num_sections,
        }
    }

    #[must_use]
    pub fn find_corridor_size(
        collector: &StructurePiecesCollector,
        random: &mut RandomGenerator,
        foot_x: i32,
        foot_y: i32,
        foot_z: i32,
        direction: BlockDirection,
    ) -> Option<BlockBox> {
        let max_len = random.next_bounded_i32(3) + 2;
        for corridor_length in (1..=max_len).rev() {
            let block_length = corridor_length * 5;
            let mut box_cand = match direction {
                BlockDirection::South => BlockBox::new(0, 0, 0, 2, 2, block_length - 1),
                BlockDirection::West => BlockBox::new(-(block_length - 1), 0, 0, 0, 2, 2),
                BlockDirection::East => BlockBox::new(0, 0, 0, block_length - 1, 2, 2),
                _ => BlockBox::new(0, 0, -(block_length - 1), 2, 2, 0),
            };
            box_cand.move_pos(foot_x, foot_y, foot_z);
            if collector.get_intersecting(&box_cand).is_none() {
                return Some(box_cand);
            }
        }
        None
    }

    #[expect(clippy::too_many_lines)]
    fn add_corridor_children(
        bb: &BlockBox,
        depth: u32,
        orientation: BlockDirection,
        start_piece_box: &BlockBox,
        shaft_type: MineshaftType,
        collector: &mut StructurePiecesCollector,
        random: &mut RandomGenerator,
    ) {
        let end_selection = random.next_bounded_i32(4);
        let rand_y = bb.min.y - 1 + random.next_bounded_i32(3);

        match orientation {
            BlockDirection::North => {
                if end_selection <= 1 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.min.x,
                        rand_y,
                        bb.min.z - 1,
                        orientation,
                        depth,
                        shaft_type,
                    );
                } else if end_selection == 2 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.min.x - 1,
                        rand_y,
                        bb.min.z,
                        BlockDirection::West,
                        depth,
                        shaft_type,
                    );
                } else {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.max.x + 1,
                        rand_y,
                        bb.min.z,
                        BlockDirection::East,
                        depth,
                        shaft_type,
                    );
                }
            }
            BlockDirection::South => {
                if end_selection <= 1 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.min.x,
                        rand_y,
                        bb.max.z + 1,
                        orientation,
                        depth,
                        shaft_type,
                    );
                } else if end_selection == 2 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.min.x - 1,
                        rand_y,
                        bb.max.z - 3,
                        BlockDirection::West,
                        depth,
                        shaft_type,
                    );
                } else {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.max.x + 1,
                        rand_y,
                        bb.max.z - 3,
                        BlockDirection::East,
                        depth,
                        shaft_type,
                    );
                }
            }
            BlockDirection::West => {
                if end_selection <= 1 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.min.x - 1,
                        rand_y,
                        bb.min.z,
                        orientation,
                        depth,
                        shaft_type,
                    );
                } else if end_selection == 2 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.min.x,
                        rand_y,
                        bb.min.z - 1,
                        BlockDirection::North,
                        depth,
                        shaft_type,
                    );
                } else {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.min.x,
                        rand_y,
                        bb.max.z + 1,
                        BlockDirection::South,
                        depth,
                        shaft_type,
                    );
                }
            }
            BlockDirection::East => {
                if end_selection <= 1 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.max.x + 1,
                        rand_y,
                        bb.min.z,
                        orientation,
                        depth,
                        shaft_type,
                    );
                } else if end_selection == 2 {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.max.x - 3,
                        rand_y,
                        bb.min.z - 1,
                        BlockDirection::North,
                        depth,
                        shaft_type,
                    );
                } else {
                    generate_and_add_piece(
                        start_piece_box,
                        collector,
                        random,
                        bb.max.x - 3,
                        rand_y,
                        bb.max.z + 1,
                        BlockDirection::South,
                        depth,
                        shaft_type,
                    );
                }
            }
            _ => {}
        }

        if depth < 8 {
            if orientation != BlockDirection::North && orientation != BlockDirection::South {
                let mut x = bb.min.x + 3;
                while x + 3 <= bb.max.x {
                    let sel = random.next_bounded_i32(5);
                    if sel == 0 {
                        generate_and_add_piece(
                            start_piece_box,
                            collector,
                            random,
                            x,
                            bb.min.y,
                            bb.min.z - 1,
                            BlockDirection::North,
                            depth + 1,
                            shaft_type,
                        );
                    } else if sel == 1 {
                        generate_and_add_piece(
                            start_piece_box,
                            collector,
                            random,
                            x,
                            bb.min.y,
                            bb.max.z + 1,
                            BlockDirection::South,
                            depth + 1,
                            shaft_type,
                        );
                    }
                    x += 5;
                }
            } else {
                let mut z = bb.min.z + 3;
                while z + 3 <= bb.max.z {
                    let sel = random.next_bounded_i32(5);
                    if sel == 0 {
                        generate_and_add_piece(
                            start_piece_box,
                            collector,
                            random,
                            bb.min.x - 1,
                            bb.min.y,
                            z,
                            BlockDirection::West,
                            depth + 1,
                            shaft_type,
                        );
                    } else if sel == 1 {
                        generate_and_add_piece(
                            start_piece_box,
                            collector,
                            random,
                            bb.max.x + 1,
                            bb.min.y,
                            z,
                            BlockDirection::East,
                            depth + 1,
                            shaft_type,
                        );
                    }
                    z += 5;
                }
            }
        }
    }

    fn create_chest(
        placer: &ShaftPlacer<'_>,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let pos = placer.piece.offset_pos(x, y, z);
        if !placer.chunk_box.contains_pos(&pos)
            || !chunk.get_block_state(&pos).to_state().is_air()
            || chunk
                .get_block_state(&Vector3::new(pos.x, pos.y - 1, pos.z))
                .to_state()
                .is_air()
        {
            return;
        }
        let shape = if random.next_bool() {
            RailShape::NorthSouth
        } else {
            RailShape::EastWest
        };
        placer.place_block(chunk, rail(shape), x, y, z);

        let mut nbt = NbtCompound::new();
        nbt.put_string("id", "minecraft:chest_minecart".to_string());
        nbt.put(
            "Pos",
            NbtTag::List(vec![
                (f64::from(pos.x) + 0.5).into(),
                (f64::from(pos.y) + 0.5).into(),
                (f64::from(pos.z) + 0.5).into(),
            ]),
        );
        nbt.put(
            "Motion",
            NbtTag::List(vec![0.0f64.into(), 0.0f64.into(), 0.0f64.into()]),
        );
        nbt.put("Rotation", NbtTag::List(vec![0.0f32.into(), 0.0f32.into()]));
        nbt.put_string(
            "LootTable",
            "minecraft:chests/abandoned_mineshaft".to_string(),
        );
        nbt.put_long("LootTableSeed", random.next_i64());
        chunk.add_structure_entity(nbt);
    }

    #[expect(clippy::too_many_arguments)]
    fn place_support(
        placer: &ShaftPlacer<'_>,
        chunk: &mut ProtoChunk,
        x0: i32,
        y0: i32,
        z: i32,
        y1: i32,
        x1: i32,
        random: &mut RandomGenerator,
    ) {
        if !placer.is_supporting_box(chunk, x0, x1, y1, z) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        let planks = placer.shaft_type.planks();
        let fence_block = Block::from_state_id(placer.shaft_type.fence().id);
        let mut west = OakFenceLikeProperties::default(fence_block);
        west.west = true;
        let mut east = OakFenceLikeProperties::default(fence_block);
        east.east = true;
        let west = BlockState::from_id(west.to_state_id(fence_block));
        let east = BlockState::from_id(east.to_state_id(fence_block));

        placer.generate_box(chunk, x0, y0, z, x0, y1 - 1, z, west, air);
        placer.generate_box(chunk, x1, y0, z, x1, y1 - 1, z, east, air);
        if random.next_bounded_i32(4) == 0 {
            placer.generate_box(chunk, x0, y1, z, x0, y1, z, planks, air);
            placer.generate_box(chunk, x1, y1, z, x1, y1, z, planks, air);
        } else {
            placer.generate_box(chunk, x0, y1, z, x1, y1, z, planks, air);
            placer.maybe_generate_block(
                chunk,
                random,
                0.05,
                x0 + 1,
                y1,
                z - 1,
                wall_torch(HorizontalFacing::South),
            );
            placer.maybe_generate_block(
                chunk,
                random,
                0.05,
                x0 + 1,
                y1,
                z + 1,
                wall_torch(HorizontalFacing::North),
            );
        }
    }

    fn maybe_place_cobweb(
        placer: &ShaftPlacer<'_>,
        chunk: &mut ProtoChunk,
        random: &mut RandomGenerator,
        probability: f32,
        x: i32,
        y: i32,
        z: i32,
    ) {
        if placer.is_interior(chunk, x, y, z)
            && random.next_f32() < probability
            && Self::has_sturdy_neighbours(placer, chunk, x, y, z, 2)
        {
            placer.place_block(chunk, Block::COBWEB.default_state, x, y, z);
        }
    }

    fn has_sturdy_neighbours(
        placer: &ShaftPlacer<'_>,
        chunk: &ProtoChunk,
        x: i32,
        y: i32,
        z: i32,
        count: usize,
    ) -> bool {
        let pos = placer.piece.offset_pos(x, y, z);
        pumpkin_data::BlockDirection::all()
            .iter()
            .filter(|direction| {
                let neighbour = pos + direction.to_offset();
                placer.chunk_box.contains_pos(&neighbour)
                    && chunk
                        .get_block_state(&neighbour)
                        .to_state()
                        .is_side_solid(direction.opposite())
            })
            .nth(count - 1)
            .is_some()
    }

    fn place_double_lower_or_upper_support(
        placer: &ShaftPlacer<'_>,
        chunk: &mut ProtoChunk,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let planks = Block::from_state_id(placer.shaft_type.planks().id);
        for x in [x, x + 2] {
            if Block::from_state_id(placer.get_block(chunk, x, y, z).id) == planks {
                Self::fill_pillar_down_or_chain_up(placer, chunk, x, y, z);
            }
        }
    }

    fn fill_pillar_down_or_chain_up(
        placer: &ShaftPlacer<'_>,
        chunk: &mut ProtoChunk,
        x: i32,
        y: i32,
        z: i32,
    ) {
        let pos = placer.piece.offset_pos(x, y, z);
        if !placer.chunk_box.contains_pos(&pos) {
            return;
        }
        let min_y = i32::from(chunk.bottom_y());
        let max_y = min_y + i32::from(chunk.height()) - 1;
        let state_at = |chunk: &ProtoChunk, y: i32| {
            chunk
                .get_block_state(&Vector3::new(pos.x, y, pos.z))
                .to_state()
        };

        let world_y = pos.y;
        let mut distance = 1;
        let mut check_below = true;
        let mut check_above = true;
        while check_below || check_above {
            if check_below {
                let y = world_y - distance;
                let below = state_at(chunk, y);
                let empty_below = is_replaceable_by_structures(below)
                    && Block::from_state_id(below.id) != &Block::LAVA;
                if !empty_below && below.is_side_solid(pumpkin_data::BlockDirection::Up) {
                    for pillar_y in (y + 1)..world_y {
                        chunk.set_block_state(pos.x, pillar_y, pos.z, placer.shaft_type.wood());
                    }
                    return;
                }
                check_below = distance <= 20 && empty_below && y > min_y + 1;
            }

            if check_above {
                let y = world_y + distance;
                let above = state_at(chunk, y);
                let empty_above = is_replaceable_by_structures(above);
                if !empty_above && can_hang_chain_below(above) {
                    chunk.set_block_state(pos.x, world_y + 1, pos.z, placer.shaft_type.fence());
                    for chain_y in (world_y + 2)..y {
                        chunk.set_block_state(
                            pos.x,
                            chain_y,
                            pos.z,
                            Block::IRON_CHAIN.default_state,
                        );
                    }
                    return;
                }
                check_above = distance <= 50 && empty_above && y < max_y;
            }

            distance += 1;
        }
    }
}

impl StructurePieceBase for MineShaftCorridor {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }

    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let placer = ShaftPlacer {
            piece: &self.piece,
            shaft_type: self.shaft_type,
            chunk_box,
        };
        if placer.is_in_invalid_location(chunk) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        let cobweb = Block::COBWEB.default_state;
        let length = self.num_sections * 5 - 1;

        placer.generate_box(chunk, 0, 0, 0, 2, 1, length, air, air);
        placer.generate_maybe_box(chunk, random, 0.8, 0, 2, 0, 2, 2, length, air, air, false);
        if self.spider_corridor {
            placer.generate_maybe_box(chunk, random, 0.6, 0, 0, 0, 2, 1, length, cobweb, air, true);
        }

        for section in 0..self.num_sections {
            let z = 2 + section * 5;
            Self::place_support(&placer, chunk, 0, 0, z, 2, 2, random);
            for (x, z, probability) in [
                (0, z - 1, 0.1),
                (2, z - 1, 0.1),
                (0, z + 1, 0.1),
                (2, z + 1, 0.1),
                (0, z - 2, 0.05),
                (2, z - 2, 0.05),
                (0, z + 2, 0.05),
                (2, z + 2, 0.05),
            ] {
                Self::maybe_place_cobweb(&placer, chunk, random, probability, x, 2, z);
            }
            if random.next_bounded_i32(100) == 0 {
                Self::create_chest(&placer, chunk, random, 2, 0, z - 1);
            }
            if random.next_bounded_i32(100) == 0 {
                Self::create_chest(&placer, chunk, random, 0, 0, z + 1);
            }

            if self.spider_corridor && !self.has_placed_spider {
                let spawner_z = z - 1 + random.next_bounded_i32(3);
                let pos = self.piece.offset_pos(1, 0, spawner_z);
                if chunk_box.contains_pos(&pos) && placer.is_interior(chunk, 1, 0, spawner_z) {
                    self.has_placed_spider = true;
                    chunk.set_block_state(pos.x, pos.y, pos.z, Block::SPAWNER.default_state);

                    let mut nbt = NbtCompound::new();
                    nbt.put_string("id", "minecraft:mob_spawner".to_string());
                    nbt.put_int("x", pos.x);
                    nbt.put_int("y", pos.y);
                    nbt.put_int("z", pos.z);
                    let mut spawn_data = NbtCompound::new();
                    let mut entity = NbtCompound::new();
                    entity.put_string("id", "minecraft:cave_spider".to_string());
                    spawn_data.put_compound("entity", entity);
                    nbt.put_compound("SpawnData", spawn_data);
                    chunk.add_block_entity(nbt);
                }
            }
        }

        for x in 0..=2 {
            for z in 0..=length {
                placer.set_planks_block(chunk, x, -1, z);
            }
        }

        Self::place_double_lower_or_upper_support(&placer, chunk, 0, -1, 2);
        if self.num_sections > 1 {
            Self::place_double_lower_or_upper_support(&placer, chunk, 0, -1, length - 2);
        }

        if self.has_rails {
            let rail = rail(RailShape::NorthSouth);
            for z in 0..=length {
                let floor = placer.get_block(chunk, 1, -1, z);
                if !floor.is_air() && floor.is_solid_render() {
                    let probability = if placer.is_interior(chunk, 1, 0, z) {
                        0.7
                    } else {
                        0.9
                    };
                    placer.maybe_generate_block(chunk, random, probability, 1, 0, z, rail);
                }
            }
        }
    }
}

pub struct MineShaftCrossing {
    pub piece: StructurePiece,
    pub shaft_type: MineshaftType,
    pub direction: Option<BlockDirection>,
    pub is_two_floored: bool,
}

impl MineShaftCrossing {
    #[must_use]
    pub const fn new(
        gen_depth: u32,
        bounding_box: BlockBox,
        direction: Option<BlockDirection>,
        shaft_type: MineshaftType,
    ) -> Self {
        let is_two_floored = bounding_box.max.y - bounding_box.min.y + 1 > 3;
        let piece = StructurePiece::new(
            StructurePieceType::MineshaftCrossing,
            bounding_box,
            gen_depth,
        );
        Self {
            piece,
            shaft_type,
            direction,
            is_two_floored,
        }
    }

    #[must_use]
    pub fn find_crossing(
        collector: &StructurePiecesCollector,
        random: &mut RandomGenerator,
        foot_x: i32,
        foot_y: i32,
        foot_z: i32,
        direction: BlockDirection,
    ) -> Option<BlockBox> {
        let y1 = if random.next_bounded_i32(4) == 0 {
            6
        } else {
            2
        };
        let mut box_cand = match direction {
            BlockDirection::South => BlockBox::new(-1, 0, 0, 3, y1, 4),
            BlockDirection::West => BlockBox::new(-4, 0, -1, 0, y1, 3),
            BlockDirection::East => BlockBox::new(0, 0, -1, 4, y1, 3),
            _ => BlockBox::new(-1, 0, -4, 3, y1, 0),
        };
        box_cand.move_pos(foot_x, foot_y, foot_z);
        collector
            .get_intersecting(&box_cand)
            .is_none()
            .then_some(box_cand)
    }

    #[expect(clippy::too_many_arguments, clippy::too_many_lines)]
    fn add_crossing_children(
        bb: &BlockBox,
        depth: u32,
        direction: Option<BlockDirection>,
        is_two_floored: bool,
        start_piece_box: &BlockBox,
        shaft_type: MineshaftType,
        collector: &mut StructurePiecesCollector,
        random: &mut RandomGenerator,
    ) {
        let dir = direction.unwrap_or(BlockDirection::North);

        match dir {
            BlockDirection::North => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y,
                    bb.min.z - 1,
                    BlockDirection::North,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x - 1,
                    bb.min.y,
                    bb.min.z + 1,
                    BlockDirection::West,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.max.x + 1,
                    bb.min.y,
                    bb.min.z + 1,
                    BlockDirection::East,
                    depth,
                    shaft_type,
                );
            }
            BlockDirection::South => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y,
                    bb.max.z + 1,
                    BlockDirection::South,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x - 1,
                    bb.min.y,
                    bb.min.z + 1,
                    BlockDirection::West,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.max.x + 1,
                    bb.min.y,
                    bb.min.z + 1,
                    BlockDirection::East,
                    depth,
                    shaft_type,
                );
            }
            BlockDirection::West => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y,
                    bb.min.z - 1,
                    BlockDirection::North,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y,
                    bb.max.z + 1,
                    BlockDirection::South,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x - 1,
                    bb.min.y,
                    bb.min.z + 1,
                    BlockDirection::West,
                    depth,
                    shaft_type,
                );
            }
            BlockDirection::East => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y,
                    bb.min.z - 1,
                    BlockDirection::North,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y,
                    bb.max.z + 1,
                    BlockDirection::South,
                    depth,
                    shaft_type,
                );
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.max.x + 1,
                    bb.min.y,
                    bb.min.z + 1,
                    BlockDirection::East,
                    depth,
                    shaft_type,
                );
            }
            _ => {}
        }

        if is_two_floored {
            if random.next_bool() {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y + 4,
                    bb.min.z - 1,
                    BlockDirection::North,
                    depth,
                    shaft_type,
                );
            }
            if random.next_bool() {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x - 1,
                    bb.min.y + 4,
                    bb.min.z + 1,
                    BlockDirection::West,
                    depth,
                    shaft_type,
                );
            }
            if random.next_bool() {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.max.x + 1,
                    bb.min.y + 4,
                    bb.min.z + 1,
                    BlockDirection::East,
                    depth,
                    shaft_type,
                );
            }
            if random.next_bool() {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x + 1,
                    bb.min.y + 4,
                    bb.max.z + 1,
                    BlockDirection::South,
                    depth,
                    shaft_type,
                );
            }
        }
    }

    fn place_support_pillar(
        placer: &ShaftPlacer<'_>,
        chunk: &mut ProtoChunk,
        x: i32,
        y0: i32,
        z: i32,
        y1: i32,
    ) {
        if !placer.get_block(chunk, x, y1 + 1, z).is_air() {
            let planks = placer.shaft_type.planks();
            placer.generate_box(
                chunk,
                x,
                y0,
                z,
                x,
                y1,
                z,
                planks,
                Block::CAVE_AIR.default_state,
            );
        }
    }
}

impl StructurePieceBase for MineShaftCrossing {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }

    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn WorldPortalExt,
        _random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let placer = ShaftPlacer {
            piece: &self.piece,
            shaft_type: self.shaft_type,
            chunk_box,
        };
        if placer.is_in_invalid_location(chunk) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        let bb = self.piece.bounding_box;

        if self.is_two_floored {
            let (x0, y0, z0) = (bb.min.x, bb.min.y, bb.min.z);
            let (x1, y1, z1) = (bb.max.x, bb.max.y, bb.max.z);
            placer.generate_box(chunk, x0 + 1, y0, z0, x1 - 1, y0 + 2, z1, air, air);
            placer.generate_box(chunk, x0, y0, z0 + 1, x1, y0 + 2, z1 - 1, air, air);
            placer.generate_box(chunk, x0 + 1, y1 - 2, z0, x1 - 1, y1, z1, air, air);
            placer.generate_box(chunk, x0, y1 - 2, z0 + 1, x1, y1, z1 - 1, air, air);
            placer.generate_box(
                chunk,
                x0 + 1,
                y0 + 3,
                z0 + 1,
                x1 - 1,
                y0 + 3,
                z1 - 1,
                air,
                air,
            );
        } else {
            placer.generate_box(
                chunk,
                bb.min.x + 1,
                bb.min.y,
                bb.min.z,
                bb.max.x - 1,
                bb.max.y,
                bb.max.z,
                air,
                air,
            );
            placer.generate_box(
                chunk,
                bb.min.x,
                bb.min.y,
                bb.min.z + 1,
                bb.max.x,
                bb.max.y,
                bb.max.z - 1,
                air,
                air,
            );
        }

        for (x, z) in [
            (bb.min.x + 1, bb.min.z + 1),
            (bb.min.x + 1, bb.max.z - 1),
            (bb.max.x - 1, bb.min.z + 1),
            (bb.max.x - 1, bb.max.z - 1),
        ] {
            Self::place_support_pillar(&placer, chunk, x, bb.min.y, z, bb.max.y);
        }

        let y = bb.min.y - 1;
        for x in bb.min.x..=bb.max.x {
            for z in bb.min.z..=bb.max.z {
                placer.set_planks_block(chunk, x, y, z);
            }
        }
    }
}

pub struct MineShaftStairs {
    pub piece: StructurePiece,
    pub shaft_type: MineshaftType,
}

impl MineShaftStairs {
    #[must_use]
    pub const fn new(
        gen_depth: u32,
        bounding_box: BlockBox,
        direction: BlockDirection,
        shaft_type: MineshaftType,
    ) -> Self {
        let mut piece =
            StructurePiece::new(StructurePieceType::MineshaftStairs, bounding_box, gen_depth);
        piece.set_facing(Some(direction));
        Self { piece, shaft_type }
    }

    #[must_use]
    pub fn find_stairs(
        collector: &StructurePiecesCollector,
        _random: &mut RandomGenerator,
        foot_x: i32,
        foot_y: i32,
        foot_z: i32,
        direction: BlockDirection,
    ) -> Option<BlockBox> {
        let mut box_cand = match direction {
            BlockDirection::South => BlockBox::new(0, -5, 0, 2, 2, 8),
            BlockDirection::West => BlockBox::new(-8, -5, 0, 0, 2, 2),
            BlockDirection::East => BlockBox::new(0, -5, 0, 8, 2, 2),
            _ => BlockBox::new(0, -5, -8, 2, 2, 0),
        };
        box_cand.move_pos(foot_x, foot_y, foot_z);
        collector
            .get_intersecting(&box_cand)
            .is_none()
            .then_some(box_cand)
    }

    fn add_stairs_children(
        bb: &BlockBox,
        depth: u32,
        direction: Option<BlockDirection>,
        start_piece_box: &BlockBox,
        shaft_type: MineshaftType,
        collector: &mut StructurePiecesCollector,
        random: &mut RandomGenerator,
    ) {
        let dir = direction.unwrap_or(BlockDirection::North);

        match dir {
            BlockDirection::North => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x,
                    bb.min.y,
                    bb.min.z - 1,
                    BlockDirection::North,
                    depth,
                    shaft_type,
                );
            }
            BlockDirection::South => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x,
                    bb.min.y,
                    bb.max.z + 1,
                    BlockDirection::South,
                    depth,
                    shaft_type,
                );
            }
            BlockDirection::West => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.min.x - 1,
                    bb.min.y,
                    bb.min.z,
                    BlockDirection::West,
                    depth,
                    shaft_type,
                );
            }
            BlockDirection::East => {
                generate_and_add_piece(
                    start_piece_box,
                    collector,
                    random,
                    bb.max.x + 1,
                    bb.min.y,
                    bb.min.z,
                    BlockDirection::East,
                    depth,
                    shaft_type,
                );
            }
            _ => {}
        }
    }
}

impl StructurePieceBase for MineShaftStairs {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece
    }

    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn WorldPortalExt,
        _random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let placer = ShaftPlacer {
            piece: &self.piece,
            shaft_type: self.shaft_type,
            chunk_box,
        };
        if placer.is_in_invalid_location(chunk) {
            return;
        }
        let air = Block::CAVE_AIR.default_state;
        placer.generate_box(chunk, 0, 5, 0, 2, 7, 1, air, air);
        placer.generate_box(chunk, 0, 0, 7, 2, 2, 8, air, air);

        for i in 0..5 {
            let y_min = 5 - i - i32::from(i < 4);
            placer.generate_box(chunk, 0, y_min, 2 + i, 2, 7 - i, 2 + i, air, air);
        }
    }
}
