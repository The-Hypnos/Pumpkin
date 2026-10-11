use pumpkin_data::{
    Block, BlockState,
    block_properties::{HorizontalFacing, LadderLikeProperties, OakFenceLikeProperties},
};
use pumpkin_util::{
    BlockDirection,
    math::block_box::BlockBox,
    random::{RandomGenerator, RandomImpl},
};

use crate::{
    ProtoChunk,
    generation::structure::{
        piece::StructurePieceType,
        structures::{
            StructurePiece, StructurePieceBase, StructurePiecesCollector,
            stronghold::{EntranceType, StoneBrickRandomizer, StrongholdPiece, wall_torch},
        },
    },
};

#[expect(clippy::fn_params_excessive_bools)]
fn oak_fence(north: bool, east: bool, south: bool, west: bool) -> &'static BlockState {
    let mut props = OakFenceLikeProperties::default(&Block::OAK_FENCE);
    props.north = north;
    props.east = east;
    props.south = south;
    props.west = west;
    BlockState::from_id(props.to_state_id(&Block::OAK_FENCE))
}

pub struct LibraryPiece {
    pub piece: StrongholdPiece,
    pub tall: bool,
}

impl LibraryPiece {
    pub fn create(
        collector: &mut StructurePiecesCollector,
        random: &mut impl RandomImpl,
        x: i32,
        y: i32,
        z: i32,
        orientation: BlockDirection,
        chain_length: u32,
    ) -> Option<Box<dyn StructurePieceBase>> {
        let mut bounding_box = BlockBox::rotated(x, y, z, -4, -1, 0, 14, 11, 15, &orientation);

        if !StrongholdPiece::is_in_bounds(&bounding_box)
            || collector.get_intersecting(&bounding_box).is_some()
        {
            bounding_box = BlockBox::rotated(x, y, z, -4, -1, 0, 14, 6, 15, &orientation);
            if !StrongholdPiece::is_in_bounds(&bounding_box)
                || collector.get_intersecting(&bounding_box).is_some()
            {
                return None;
            }
        }

        let mut piece = StrongholdPiece::new(
            StructurePieceType::StrongholdLibrary,
            chain_length,
            bounding_box,
        );
        piece.piece.set_facing(Some(orientation));
        piece.entry_door = EntranceType::get_random(random);

        let tall = bounding_box.get_block_count_y() > 6;

        Some(Box::new(Self { piece, tall }))
    }
}

impl StructurePieceBase for LibraryPiece {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn get_structure_piece(&self) -> &StructurePiece {
        &self.piece.piece
    }

    fn get_structure_piece_mut(&mut self) -> &mut StructurePiece {
        &mut self.piece.piece
    }

    #[expect(clippy::too_many_lines)]
    fn place(
        &mut self,
        chunk: &mut ProtoChunk,
        _block_registry: &dyn crate::world::WorldPortalExt,
        random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let randomizer = StoneBrickRandomizer;
        let box_limit = *chunk_box;
        let p = &self.piece;
        let inner = &p.piece;

        let max_y = if self.tall { 10 } else { 5 }; // Java: i-1 where i is 11 or 6

        // 1. Main Shell
        inner.fill_outline_random(
            0,
            0,
            0,
            13,
            max_y,
            14,
            &randomizer,
            chunk,
            true,
            random,
            &box_limit,
        );

        // 2. Entrance
        p.generate_entrance(chunk, &box_limit, self.piece.entry_door, 4, 1, 0);

        // 3. Cobwebs (Random probability placement)
        inner.fill_with_outline_under_sea_level(
            chunk,
            &box_limit,
            random,
            0.07,
            2,
            1,
            1,
            11,
            4,
            13,
            Block::COBWEB.default_state,
            Block::COBWEB.default_state,
            false,
            false,
        );

        // 4. Bookshelves, Pillars, and Wall Torches
        let planks = Block::OAK_PLANKS.default_state;
        let books = Block::BOOKSHELF.default_state;
        let torch_e = wall_torch(HorizontalFacing::East);
        let torch_w = wall_torch(HorizontalFacing::West);

        for l in 1..=13 {
            if (l - 1) % 4 == 0 {
                // Pillars
                inner.fill_with_outline(chunk, &box_limit, false, 1, 1, l, 1, 4, l, planks, planks);
                inner.fill_with_outline(
                    chunk, &box_limit, false, 12, 1, l, 12, 4, l, planks, planks,
                );
                inner.add_block(chunk, torch_e, 2, 3, l, &box_limit);
                inner.add_block(chunk, torch_w, 11, 3, l, &box_limit);

                if self.tall {
                    inner.fill_with_outline(
                        chunk, &box_limit, false, 1, 6, l, 1, 9, l, planks, planks,
                    );
                    inner.fill_with_outline(
                        chunk, &box_limit, false, 12, 6, l, 12, 9, l, planks, planks,
                    );
                }
            } else {
                // Shelf Walls
                inner.fill_with_outline(chunk, &box_limit, false, 1, 1, l, 1, 4, l, books, books);
                inner.fill_with_outline(chunk, &box_limit, false, 12, 1, l, 12, 4, l, books, books);
                if self.tall {
                    inner.fill_with_outline(
                        chunk, &box_limit, false, 1, 6, l, 1, 9, l, books, books,
                    );
                    inner.fill_with_outline(
                        chunk, &box_limit, false, 12, 6, l, 12, 9, l, books, books,
                    );
                }
            }
        }

        // 5. Central Freestanding Shelves
        for l in (3..12).step_by(2) {
            inner.fill_with_outline(chunk, &box_limit, false, 3, 1, l, 4, 3, l, books, books);
            inner.fill_with_outline(chunk, &box_limit, false, 6, 1, l, 7, 3, l, books, books);
            inner.fill_with_outline(chunk, &box_limit, false, 9, 1, l, 10, 3, l, books, books);
        }

        if self.tall {
            // Upper Walkways
            inner.fill_with_outline(chunk, &box_limit, false, 1, 5, 1, 3, 5, 13, planks, planks);
            inner.fill_with_outline(
                chunk, &box_limit, false, 10, 5, 1, 12, 5, 13, planks, planks,
            );
            inner.fill_with_outline(chunk, &box_limit, false, 4, 5, 1, 9, 5, 2, planks, planks);
            inner.fill_with_outline(chunk, &box_limit, false, 4, 5, 12, 9, 5, 13, planks, planks);

            // Walkway corners
            inner.add_block(chunk, planks, 9, 5, 11, &box_limit);
            inner.add_block(chunk, planks, 8, 5, 11, &box_limit);
            inner.add_block(chunk, planks, 9, 5, 10, &box_limit);

            let we_fence = oak_fence(false, true, false, true);
            let ns_fence = oak_fence(true, false, true, false);
            inner.fill_with_outline(
                chunk, &box_limit, false, 3, 6, 3, 3, 6, 11, ns_fence, ns_fence,
            );
            inner.fill_with_outline(
                chunk, &box_limit, false, 10, 6, 3, 10, 6, 9, ns_fence, ns_fence,
            );
            inner.fill_with_outline(
                chunk, &box_limit, false, 4, 6, 2, 9, 6, 2, we_fence, we_fence,
            );
            inner.fill_with_outline(
                chunk, &box_limit, false, 4, 6, 12, 7, 6, 12, we_fence, we_fence,
            );
            inner.add_block(
                chunk,
                oak_fence(true, true, false, false),
                3,
                6,
                2,
                &box_limit,
            );
            inner.add_block(
                chunk,
                oak_fence(false, true, true, false),
                3,
                6,
                12,
                &box_limit,
            );
            inner.add_block(
                chunk,
                oak_fence(true, false, false, true),
                10,
                6,
                2,
                &box_limit,
            );
            for i in 0..=2 {
                inner.add_block(
                    chunk,
                    oak_fence(false, false, true, true),
                    8 + i,
                    6,
                    12 - i,
                    &box_limit,
                );
                if i != 2 {
                    inner.add_block(
                        chunk,
                        oak_fence(true, true, false, false),
                        8 + i,
                        6,
                        11 - i,
                        &box_limit,
                    );
                }
            }

            let mut props = LadderLikeProperties::default(&Block::LADDER);
            props.facing = HorizontalFacing::South;
            let ladder = BlockState::from_id(props.to_state_id(&Block::LADDER));
            for y in 1..=7 {
                inner.add_block(chunk, ladder, 10, y, 13, &box_limit);
            }

            let e_fence = oak_fence(false, true, false, false);
            let w_fence = oak_fence(false, false, false, true);
            inner.add_block(chunk, e_fence, 6, 9, 7, &box_limit);
            inner.add_block(chunk, w_fence, 7, 9, 7, &box_limit);
            inner.add_block(chunk, e_fence, 6, 8, 7, &box_limit);
            inner.add_block(chunk, w_fence, 7, 8, 7, &box_limit);
            let nswe_fence = oak_fence(true, true, true, true);
            inner.add_block(chunk, nswe_fence, 6, 7, 7, &box_limit);
            inner.add_block(chunk, nswe_fence, 7, 7, 7, &box_limit);
            inner.add_block(chunk, e_fence, 5, 7, 7, &box_limit);
            inner.add_block(chunk, w_fence, 8, 7, 7, &box_limit);
            inner.add_block(
                chunk,
                oak_fence(true, true, false, false),
                6,
                7,
                6,
                &box_limit,
            );
            inner.add_block(
                chunk,
                oak_fence(false, true, true, false),
                6,
                7,
                8,
                &box_limit,
            );
            inner.add_block(
                chunk,
                oak_fence(true, false, false, true),
                7,
                7,
                6,
                &box_limit,
            );
            inner.add_block(
                chunk,
                oak_fence(false, false, true, true),
                7,
                7,
                8,
                &box_limit,
            );

            let t = Block::TORCH.default_state;
            inner.add_block(chunk, t, 5, 8, 7, &box_limit);
            inner.add_block(chunk, t, 8, 8, 7, &box_limit);
            inner.add_block(chunk, t, 6, 8, 6, &box_limit);
            inner.add_block(chunk, t, 6, 8, 8, &box_limit);
            inner.add_block(chunk, t, 7, 8, 6, &box_limit);
            inner.add_block(chunk, t, 7, 8, 8, &box_limit);
        }

        // 6. Chests
        inner.add_chest(
            chunk,
            &box_limit,
            random,
            3,
            3,
            5,
            "minecraft:chests/stronghold_library",
        );
        if self.tall {
            inner.add_block(chunk, Block::CAVE_AIR.default_state, 12, 9, 1, &box_limit); // Clear space above top chest
            inner.add_chest(
                chunk,
                &box_limit,
                random,
                12,
                8,
                1,
                "minecraft:chests/stronghold_library",
            );
        }
    }
}
