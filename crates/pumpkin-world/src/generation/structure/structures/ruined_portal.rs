use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, Mirror, Rotation,
    block_properties::blocks_movement,
    structures::{
        RuinedPortalSetup, RuinedPortalVerticalPlacement as VerticalPlacement, Structure,
        StructureKeys,
    },
    tag,
};
use pumpkin_util::{
    HeightMap,
    math::{block_box::BlockBox, position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl, hash_block_pos, legacy_rand::LegacyRand},
};

use crate::{
    ProtoChunk,
    generation::{
        biome_coords,
        positions::chunk_pos::{start_block_x, start_block_z},
        proto_chunk::GenerationCache,
        structure::{
            piece::StructurePieceType,
            structures::{
                HeightSampler, StructureGenerator, StructureGeneratorContext, StructurePiece,
                StructurePieceBase, StructurePiecesCollector, StructurePosition, WorldPortalExt,
            },
            template::{
                BlockStateResolver, PaletteEntry, RegionPlacer, StructurePlaceSettings,
                StructureTemplate, get_template, holds_water_source,
                processor::{
                    IgnoredBlock, PosRuleTest, ProcessorContext, ProcessorRule, RuleTest,
                    StructureProcessor,
                },
            },
        },
    },
};

const STRUCTURE_LOCATION_PORTALS: &[&str] = &[
    "ruined_portal/portal_1",
    "ruined_portal/portal_2",
    "ruined_portal/portal_3",
    "ruined_portal/portal_4",
    "ruined_portal/portal_5",
    "ruined_portal/portal_6",
    "ruined_portal/portal_7",
    "ruined_portal/portal_8",
    "ruined_portal/portal_9",
    "ruined_portal/portal_10",
];

const STRUCTURE_LOCATION_GIANT_PORTALS: &[&str] = &[
    "ruined_portal/giant_portal_1",
    "ruined_portal/giant_portal_2",
    "ruined_portal/giant_portal_3",
];

const PROBABILITY_OF_GIANT_PORTAL: f32 = 0.05;

const NETHERRACK_PROBABILITY_BY_DISTANCE: &[f32] = &[
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.9, 0.9, 0.8, 0.7, 0.6, 0.4, 0.2,
];

const fn get_height_map_type(vertical_placement: VerticalPlacement) -> HeightMap {
    match vertical_placement {
        VerticalPlacement::OnOceanFloor => HeightMap::OceanFloorWg,
        _ => HeightMap::WorldSurfaceWg,
    }
}

#[derive(Clone, Debug)]
pub struct RuinedPortalProperties {
    pub cold: bool,
    pub mossiness: f32,
    pub air_pocket: bool,
    pub overgrown: bool,
    pub vines: bool,
    pub replace_with_blackstone: bool,
}

pub struct RuinedPortalGenerator {
    pub variant: StructureKeys,
}

impl StructureGenerator for RuinedPortalGenerator {
    fn get_structure_position(
        &self,
        mut context: StructureGeneratorContext<'_>,
    ) -> Option<StructurePosition> {
        let setups = Structure::get(&self.variant).ruined_portal_setups;
        let setup: RuinedPortalSetup = if setups.len() > 1 {
            let total: f32 = setups.iter().map(|setup| setup.weight).sum();
            let mut pick = context.random.next_f32();
            *setups.iter().find(|setup| {
                pick -= setup.weight / total;
                pick < 0.0
            })?
        } else {
            *setups.first()?
        };

        let air_pocket = sample(&mut context.random, setup.air_pocket_probability);
        let template_name = if context.random.next_f32() < PROBABILITY_OF_GIANT_PORTAL {
            STRUCTURE_LOCATION_GIANT_PORTALS[context
                .random
                .next_bounded_i32(STRUCTURE_LOCATION_GIANT_PORTALS.len() as i32)
                as usize]
        } else {
            STRUCTURE_LOCATION_PORTALS[context
                .random
                .next_bounded_i32(STRUCTURE_LOCATION_PORTALS.len() as i32)
                as usize]
        };
        let template = get_template(template_name)?;
        let rotation = Rotation::from_index(context.random.next_bounded_i32(4) as u8);
        let mirror = if context.random.next_f32() < 0.5 {
            Mirror::None
        } else {
            Mirror::FrontBack
        };
        let pivot = Vector3::new(template.size.x / 2, 0, template.size.z / 2);
        let base_position = Vector3::new(
            start_block_x(context.chunk_x),
            0,
            start_block_z(context.chunk_z),
        );
        let bounding_box =
            template.get_bounding_box_with_pivot(base_position, rotation, pivot, mirror);
        let center = bounding_box.get_center();

        let sampler = context.height_sampler.as_deref_mut()?;
        let surface_y = match get_height_map_type(setup.placement) {
            HeightMap::OceanFloorWg => sampler.estimate_ocean_floor_height(center.x, center.z),
            _ => sampler.estimate_height(center.x, center.z) - 1,
        };
        let projected_y = find_suitable_y(
            &mut context.random,
            sampler,
            setup.placement,
            air_pocket,
            surface_y,
            bounding_box.get_block_count_y(),
            &bounding_box,
            context.min_y,
        );
        let origin = Vector3::new(base_position.x, projected_y, base_position.z);

        let properties = RuinedPortalProperties {
            cold: false,
            mossiness: setup.mossiness,
            air_pocket,
            overgrown: setup.overgrown,
            vines: setup.vines,
            replace_with_blackstone: setup.replace_with_blackstone,
        };
        let piece = RuinedPortalPiece::new(
            template,
            template_name.to_string(),
            origin,
            setup.placement,
            properties,
            setup.can_be_cold,
            rotation,
            mirror,
            pivot,
        );

        let mut collector = StructurePiecesCollector::default();
        collector.add_piece(Box::new(piece));

        Some(StructurePosition {
            start_pos: BlockPos(origin),
            collector: Arc::new(collector.into()),
        })
    }
}

fn sample(random: &mut RandomGenerator, limit: f32) -> bool {
    if limit == 0.0 {
        false
    } else if limit == 1.0 {
        true
    } else {
        random.next_f32() < limit
    }
}

fn random_between_inclusive(random: &mut RandomGenerator, min: i32, max: i32) -> i32 {
    random.next_bounded_i32(max - min + 1) + min
}

fn get_random_within_interval(random: &mut RandomGenerator, min_preferred: i32, max: i32) -> i32 {
    if min_preferred < max {
        random_between_inclusive(random, min_preferred, max)
    } else {
        max
    }
}

#[expect(clippy::too_many_arguments)]
fn find_suitable_y(
    random: &mut RandomGenerator,
    sampler: &mut dyn HeightSampler,
    vertical_placement: VerticalPlacement,
    air_pocket: bool,
    surface_y_at_center: i32,
    y_span: i32,
    bounding_box: &BlockBox,
    min_y: i32,
) -> i32 {
    let min_y = min_y + 15;
    let new_y = match vertical_placement {
        VerticalPlacement::InNether => {
            if air_pocket {
                random_between_inclusive(random, 32, 100)
            } else if random.next_f32() < 0.5 {
                random_between_inclusive(random, 27, 29)
            } else {
                random_between_inclusive(random, 29, 100)
            }
        }
        VerticalPlacement::InMountain => {
            get_random_within_interval(random, 70, surface_y_at_center - y_span)
        }
        VerticalPlacement::Underground => {
            get_random_within_interval(random, min_y, surface_y_at_center - y_span)
        }
        VerticalPlacement::PartlyBuried => {
            surface_y_at_center - y_span + random_between_inclusive(random, 2, 8)
        }
        VerticalPlacement::OnLandSurface | VerticalPlacement::OnOceanFloor => surface_y_at_center,
    };

    let bottom_corners = [
        (bounding_box.min.x, bounding_box.min.z),
        (bounding_box.max.x, bounding_box.min.z),
        (bounding_box.min.x, bounding_box.max.z),
        (bounding_box.max.x, bounding_box.max.z),
    ];
    let ocean_floor = vertical_placement == VerticalPlacement::OnOceanFloor;
    let mut projected_y = new_y;
    while projected_y > min_y {
        let mut corners_on_solid_ground = 0;
        for &(x, z) in &bottom_corners {
            let opaque = sampler
                .sample_column_block(x, z, projected_y)
                .is_some_and(|state| {
                    if ocean_floor {
                        blocks_movement(state, state.id.to_block_id())
                    } else {
                        !state.is_air()
                    }
                });
            if opaque {
                corners_on_solid_ground += 1;
                if corners_on_solid_ground == 3 {
                    return projected_y;
                }
            }
        }
        projected_y -= 1;
    }
    projected_y
}

pub struct RuinedPortalPiece {
    pub piece: StructurePiece,
    pub template: Arc<StructureTemplate>,
    pub template_name: String,
    pub place_settings: StructurePlaceSettings,
    pub template_position: Vector3<i32>,
    pub vertical_placement: VerticalPlacement,
    pub properties: RuinedPortalProperties,
    can_be_cold: bool,
    cold_resolved: bool,
}

impl RuinedPortalPiece {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        template: Arc<StructureTemplate>,
        template_name: String,
        template_position: Vector3<i32>,
        vertical_placement: VerticalPlacement,
        properties: RuinedPortalProperties,
        can_be_cold: bool,
        rotation: Rotation,
        mirror: Mirror,
        pivot: Vector3<i32>,
    ) -> Self {
        let place_settings =
            make_settings(mirror, rotation, vertical_placement, pivot, &properties);
        let bounding_box = template.get_bounding_box(&place_settings, template_position);

        Self {
            piece: StructurePiece::new(StructurePieceType::RuinedPortal, bounding_box, 0),
            template,
            template_name,
            place_settings,
            template_position,
            vertical_placement,
            properties,
            can_be_cold,
            cold_resolved: false,
        }
    }

    fn resolve_cold(&mut self, region: &dyn GenerationCache) {
        if self.cold_resolved {
            return;
        }
        self.cold_resolved = true;
        let origin = self.template_position;
        let Some(start_chunk) = region.get_chunk(origin.x >> 4, origin.z >> 4) else {
            return;
        };
        let biome = start_chunk.get_biome(
            biome_coords::from_block(origin.x),
            biome_coords::from_block(origin.y),
            biome_coords::from_block(origin.z),
        );
        self.properties.cold = self.can_be_cold
            && biome.weather.compute_temperature(
                f64::from(origin.x),
                origin.y,
                f64::from(origin.z),
                start_chunk.get_sea_level(),
            ) < 0.15;
        self.place_settings = make_settings(
            self.place_settings.get_mirror(),
            self.place_settings.get_rotation(),
            self.vertical_placement,
            self.place_settings.get_rotation_pivot(),
            &self.properties,
        );
    }

    fn place_blocks(
        &self,
        region: &mut dyn GenerationCache,
        random: &mut RandomGenerator,
        chunk_box: &BlockBox,
    ) {
        let rotation = self.place_settings.get_rotation();
        let mirror = self.place_settings.get_mirror();
        let pivot = self.place_settings.get_rotation_pivot();

        let mut context_rng = LegacyRand::from_seed(hash_block_pos(
            self.template_position.x,
            self.template_position.y,
            self.template_position.z,
        ) as u64);
        let mut context = ProcessorContext::new(
            self.template_position,
            self.place_settings.get_processors(),
            &mut context_rng,
        );
        let placer = RegionPlacer(region);

        for block in &self.template.blocks {
            let palette_entry = &self.template.palette[block.state as usize];

            let mut block_entity_nbt = block.nbt.clone();
            let placed_entry = palette_entry.clone();

            let Some(mut state) =
                BlockStateResolver::resolve(&placed_entry, Rotation::None, Mirror::None)
            else {
                continue;
            };

            let local_pos =
                StructureTemplate::transform_block_pos(block.pos, mirror, rotation, pivot);
            let world_pos = self.template_position + local_pos;

            if !chunk_box.contains_pos(&world_pos) {
                continue;
            }

            let mut processed_state = Some(state);
            let mut capped_idx = 0;
            for processor in self.place_settings.get_processors() {
                let Some(current_state) = processed_state else {
                    break;
                };
                processed_state = processor.process_with_context(
                    &placer,
                    world_pos,
                    current_state,
                    &mut block_entity_nbt,
                    &mut context,
                    &mut capped_idx,
                );
            }

            let Some(final_state) = processed_state else {
                continue;
            };

            state = final_state.mirror(mirror).rotate(rotation);
            if holds_water_source(GenerationCache::get_block_state(placer.0, &world_pos))
                && let Some(waterlogged) = state.set_waterlogged(true)
            {
                state = waterlogged;
            }

            GenerationCache::set_block_state(placer.0, &world_pos, state);

            let final_block = Block::from_id(state.id.to_block_id());
            let block_entity_id =
                crate::generation::structure::template::get_block_entity_id(final_block.name);
            if block_entity_nbt.is_some() || block_entity_id.is_some() {
                let fallback_id = block_entity_id.unwrap_or(final_block.name);
                let mut placed_nbt = pumpkin_nbt::compound::NbtCompound::new();

                placed_nbt.put_string("id", fallback_id.to_string());
                placed_nbt.put_int("x", world_pos.x);
                placed_nbt.put_int("y", world_pos.y);
                placed_nbt.put_int("z", world_pos.z);

                if let Some(template_nbt) = &block_entity_nbt {
                    for (key, value) in &template_nbt.child_tags {
                        if key.as_ref() != "x"
                            && key.as_ref() != "y"
                            && key.as_ref() != "z"
                            && key.as_ref() != "id"
                        {
                            placed_nbt.child_tags.insert(key.clone(), value.clone());
                        }
                    }
                }

                if block_entity_nbt.is_some() && is_randomizable_container(fallback_id) {
                    placed_nbt.put_long("LootTableSeed", random.next_i64());
                }

                GenerationCache::add_block_entity(placer.0, &world_pos, placed_nbt);
            }
        }
    }

    fn spread_netherrack(&self, random: &mut RandomGenerator, region: &mut dyn GenerationCache) {
        let follow_ground_surface = self.vertical_placement == VerticalPlacement::OnLandSurface
            || self.vertical_placement == VerticalPlacement::OnOceanFloor;
        let bb = self.piece.bounding_box;
        let center = bb.get_center();

        let max_distance = NETHERRACK_PROBABILITY_BY_DISTANCE.len() as i32;
        let average_width = (bb.max.x - bb.min.x + 1 + bb.max.z - bb.min.z + 1) / 2;
        let distance_adjustment = random.next_bounded_i32((8 - average_width / 2).max(1));

        let heightmap = get_height_map_type(self.vertical_placement);

        for x in (center.x - max_distance)..=(center.x + max_distance) {
            for z in (center.z - max_distance)..=(center.z + max_distance) {
                let distance = (x - center.x).abs() + (z - center.z).abs();
                let adjusted_distance = (distance + distance_adjustment).max(0);
                if adjusted_distance < max_distance {
                    let probability =
                        NETHERRACK_PROBABILITY_BY_DISTANCE[adjusted_distance as usize];
                    if random.next_f64() < f64::from(probability) {
                        let surface_y = region.get_top_y(&heightmap, x, z) - 1;
                        let y = if follow_ground_surface {
                            surface_y
                        } else {
                            bb.min.y.min(surface_y)
                        };
                        let pos = Vector3::new(x, y, z);
                        if (y - bb.min.y).abs() <= 3
                            && self.can_block_be_replaced_by_netherrack_or_magma(region, pos)
                        {
                            self.place_netherrack_or_magma(random, region, pos);
                            if self.properties.overgrown {
                                Self::maybe_add_leaves_above(random, region, pos);
                            }
                            self.add_netherrack_drip_column(
                                random,
                                region,
                                pos + Vector3::new(0, -1, 0),
                            );
                        }
                    }
                }
            }
        }
    }

    fn can_block_be_replaced_by_netherrack_or_magma(
        &self,
        region: &dyn GenerationCache,
        pos: Vector3<i32>,
    ) -> bool {
        let block = GenerationCache::get_block_state(region, &pos).to_block_id();
        block != Block::AIR.id
            && block != Block::OBSIDIAN.id
            && !block.has_tag(tag::Block::MINECRAFT_FEATURES_CANNOT_REPLACE)
            && (self.vertical_placement == VerticalPlacement::InNether || block != Block::LAVA.id)
    }

    fn place_netherrack_or_magma(
        &self,
        random: &mut RandomGenerator,
        region: &mut dyn GenerationCache,
        pos: Vector3<i32>,
    ) {
        if !self.can_block_be_replaced_by_netherrack_or_magma(region, pos) {
            return;
        }
        if !self.properties.cold && random.next_f32() < 0.07 {
            GenerationCache::set_block_state(region, &pos, Block::MAGMA_BLOCK.default_state);
        } else {
            GenerationCache::set_block_state(region, &pos, Block::NETHERRACK.default_state);
        }
    }

    fn add_netherrack_drip_columns_below_portal(
        &self,
        random: &mut RandomGenerator,
        region: &mut dyn GenerationCache,
    ) {
        let bb = self.piece.bounding_box;
        for x in (bb.min.x + 1)..bb.max.x {
            for z in (bb.min.z + 1)..bb.max.z {
                let pos = Vector3::new(x, bb.min.y, z);
                if GenerationCache::get_block_state(region, &pos).to_block_id()
                    == Block::NETHERRACK.id
                {
                    self.add_netherrack_drip_column(
                        random,
                        region,
                        Vector3::new(x, bb.min.y - 1, z),
                    );
                }
            }
        }
    }

    fn add_netherrack_drip_column(
        &self,
        random: &mut RandomGenerator,
        region: &mut dyn GenerationCache,
        start_pos: Vector3<i32>,
    ) {
        let mut pos = start_pos;
        self.place_netherrack_or_magma(random, region, pos);
        let mut remaining_cap = 8;
        while remaining_cap > 0 && random.next_f32() < 0.5 {
            pos.y -= 1;
            remaining_cap -= 1;
            self.place_netherrack_or_magma(random, region, pos);
        }
    }

    fn maybe_add_vines(
        random: &mut RandomGenerator,
        region: &mut dyn GenerationCache,
        pos: Vector3<i32>,
    ) {
        let state = GenerationCache::get_block_state(region, &pos).to_state();
        let block = state.id.to_block_id();
        if state.is_air() || block == Block::VINE.id {
            return;
        }
        let direction = [
            BlockDirection::North,
            BlockDirection::East,
            BlockDirection::South,
            BlockDirection::West,
        ][random.next_bounded_i32(4) as usize];
        let neighbour_pos = pos + direction.to_offset();
        if GenerationCache::get_block_state(region, &neighbour_pos)
            .to_state()
            .is_air()
            && state.is_collision_face_full(direction)
        {
            let face = match direction.opposite() {
                BlockDirection::North => "north",
                BlockDirection::East => "east",
                BlockDirection::South => "south",
                _ => "west",
            };
            let entry = PaletteEntry::with_properties(
                "minecraft:vine".to_string(),
                vec![(face.to_string(), "true".to_string())],
            );
            if let Some(vine_state) =
                BlockStateResolver::resolve(&entry, Rotation::None, Mirror::None)
            {
                GenerationCache::set_block_state(region, &neighbour_pos, vine_state);
            }
        }
    }

    fn maybe_add_leaves_above(
        random: &mut RandomGenerator,
        region: &mut dyn GenerationCache,
        pos: Vector3<i32>,
    ) {
        if random.next_f32() < 0.5
            && GenerationCache::get_block_state(region, &pos).to_block_id() == Block::NETHERRACK.id
        {
            let above = pos + Vector3::new(0, 1, 0);
            if GenerationCache::get_block_state(region, &above)
                .to_state()
                .is_air()
            {
                let entry = PaletteEntry::with_properties(
                    "minecraft:jungle_leaves".to_string(),
                    vec![("persistent".to_string(), "true".to_string())],
                );
                if let Some(leaves_state) =
                    BlockStateResolver::resolve(&entry, Rotation::None, Mirror::None)
                {
                    GenerationCache::set_block_state(region, &above, leaves_state);
                }
            }
        }
    }
}

impl StructurePieceBase for RuinedPortalPiece {
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
        _chunk: &mut ProtoChunk,
        _block_registry: &dyn WorldPortalExt,
        _random: &mut RandomGenerator,
        _seed: i64,
        _chunk_box: &BlockBox,
    ) {
    }

    fn place_in_region(
        &mut self,
        region: &mut dyn GenerationCache,
        _block_registry: &dyn WorldPortalExt,
        random: &mut RandomGenerator,
        _seed: i64,
        chunk_box: &BlockBox,
    ) {
        let bounding_box = self
            .template
            .get_bounding_box(&self.place_settings, self.template_position);
        if !chunk_box.contains_pos(&bounding_box.get_center()) {
            return;
        }
        self.resolve_cold(region);
        let mut enlarged_box = *chunk_box;
        enlarged_box.encompass(&bounding_box);

        self.place_blocks(region, random, &enlarged_box);
        self.spread_netherrack(random, region);
        self.add_netherrack_drip_columns_below_portal(random, region);

        if self.properties.vines || self.properties.overgrown {
            let bb = self.piece.bounding_box;
            for z in bb.min.z..=bb.max.z {
                for y in bb.min.y..=bb.max.y {
                    for x in bb.min.x..=bb.max.x {
                        let pos = Vector3::new(x, y, z);
                        if self.properties.vines {
                            Self::maybe_add_vines(random, region, pos);
                        }
                        if self.properties.overgrown {
                            Self::maybe_add_leaves_above(random, region, pos);
                        }
                    }
                }
            }
        }
    }
}

fn make_settings(
    mirror: Mirror,
    rotation: Rotation,
    vertical_placement: VerticalPlacement,
    pivot: Vector3<i32>,
    properties: &RuinedPortalProperties,
) -> StructurePlaceSettings {
    let ignore_processor = if properties.air_pocket {
        StructureProcessor::BlockIgnore(vec![IgnoredBlock {
            block_id: Block::STRUCTURE_BLOCK.id,
            properties: None,
        }])
    } else {
        StructureProcessor::BlockIgnore(vec![
            IgnoredBlock {
                block_id: Block::STRUCTURE_BLOCK.id,
                properties: None,
            },
            IgnoredBlock {
                block_id: Block::AIR.id,
                properties: None,
            },
        ])
    };

    let mut rules = Vec::new();
    rules.push(ProcessorRule {
        position_predicate: PosRuleTest::AlwaysTrue,
        input_predicate: RuleTest::RandomBlockMatch {
            block: Block::GOLD_BLOCK.id,
            probability: 0.3,
        },
        location_predicate: RuleTest::AlwaysTrue,
        output_state: Block::AIR.default_state,
        block_entity_modifier: None,
    });

    let lava_rule = match vertical_placement {
        VerticalPlacement::OnOceanFloor => ProcessorRule {
            position_predicate: PosRuleTest::AlwaysTrue,
            input_predicate: RuleTest::BlockMatch(Block::LAVA.id),
            location_predicate: RuleTest::AlwaysTrue,
            output_state: Block::MAGMA_BLOCK.default_state,
            block_entity_modifier: None,
        },
        _ => {
            if properties.cold {
                ProcessorRule {
                    position_predicate: PosRuleTest::AlwaysTrue,
                    input_predicate: RuleTest::BlockMatch(Block::LAVA.id),
                    location_predicate: RuleTest::AlwaysTrue,
                    output_state: Block::NETHERRACK.default_state,
                    block_entity_modifier: None,
                }
            } else {
                ProcessorRule {
                    position_predicate: PosRuleTest::AlwaysTrue,
                    input_predicate: RuleTest::RandomBlockMatch {
                        block: Block::LAVA.id,
                        probability: 0.2,
                    },
                    location_predicate: RuleTest::AlwaysTrue,
                    output_state: Block::MAGMA_BLOCK.default_state,
                    block_entity_modifier: None,
                }
            }
        }
    };
    rules.push(lava_rule);

    if !properties.cold {
        rules.push(ProcessorRule {
            position_predicate: PosRuleTest::AlwaysTrue,
            input_predicate: RuleTest::RandomBlockMatch {
                block: Block::NETHERRACK.id,
                probability: 0.07,
            },
            location_predicate: RuleTest::AlwaysTrue,
            output_state: Block::MAGMA_BLOCK.default_state,
            block_entity_modifier: None,
        });
    }

    let mut settings = StructurePlaceSettings::new()
        .set_rotation(rotation)
        .set_mirror(mirror)
        .set_rotation_pivot(pivot)
        .add_processor(ignore_processor)
        .add_processor(StructureProcessor::Rule(rules))
        .add_processor(StructureProcessor::BlockAge {
            mossiness: properties.mossiness,
        })
        .add_processor(StructureProcessor::ProtectedBlocks(
            "#minecraft:features_cannot_replace".to_string(),
        ))
        .add_processor(StructureProcessor::LavaSubmergedBlock);

    if properties.replace_with_blackstone {
        settings = settings.add_processor(StructureProcessor::BlackstoneReplace);
    }

    settings
}

fn is_randomizable_container(id: &str) -> bool {
    matches!(
        id.strip_prefix("minecraft:").unwrap_or(id),
        "chest"
            | "trapped_chest"
            | "barrel"
            | "dispenser"
            | "dropper"
            | "hopper"
            | "shulker_box"
            | "decorated_pot"
            | "crafter"
    )
}
