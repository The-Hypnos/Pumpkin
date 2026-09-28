//! Vanilla's `PoiManager`: points of interest (job sites, beds, bells and the like) indexed per
//! loaded chunk, so villager AI can query them without scanning blocks.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use dashmap::DashMap;
use pumpkin_data::block_properties::{BedPart, BedProperties};
use pumpkin_data::tag::{Tag, Taggable};
use pumpkin_data::{Block, BlockStateId, tag};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use rustc_hash::FxHashMap;

use crate::world::World;
use crate::world::portal::PortalPoiStorage;
use pumpkin_world::poi::PoiRegion;
use std::sync::Arc;

/// Vanilla `PoiManager.MAX_VILLAGE_DISTANCE`.
pub const MAX_VILLAGE_DISTANCE: i32 = 6;
const NOT_A_VILLAGE: i32 = MAX_VILLAGE_DISTANCE + 1;

/// Vanilla `PoiTypes`, in registration order so the discriminant is the registry id tags use.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u16)]
pub enum PoiType {
    Armorer,
    Butcher,
    Cartographer,
    Cleric,
    Farmer,
    Fisherman,
    Fletcher,
    Leatherworker,
    Librarian,
    Mason,
    Shepherd,
    Toolsmith,
    Weaponsmith,
    Home,
    Meeting,
    Beehive,
    BeeNest,
    NetherPortal,
    Lodestone,
    TestInstance,
    LightningRod,
}

impl PoiType {
    const ALL: [Self; 21] = [
        Self::Armorer,
        Self::Butcher,
        Self::Cartographer,
        Self::Cleric,
        Self::Farmer,
        Self::Fisherman,
        Self::Fletcher,
        Self::Leatherworker,
        Self::Librarian,
        Self::Mason,
        Self::Shepherd,
        Self::Toolsmith,
        Self::Weaponsmith,
        Self::Home,
        Self::Meeting,
        Self::Beehive,
        Self::BeeNest,
        Self::NetherPortal,
        Self::Lodestone,
        Self::TestInstance,
        Self::LightningRod,
    ];

    /// The registry name, as saved in the POI region files.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Armorer => "minecraft:armorer",
            Self::Butcher => "minecraft:butcher",
            Self::Cartographer => "minecraft:cartographer",
            Self::Cleric => "minecraft:cleric",
            Self::Farmer => "minecraft:farmer",
            Self::Fisherman => "minecraft:fisherman",
            Self::Fletcher => "minecraft:fletcher",
            Self::Leatherworker => "minecraft:leatherworker",
            Self::Librarian => "minecraft:librarian",
            Self::Mason => "minecraft:mason",
            Self::Shepherd => "minecraft:shepherd",
            Self::Toolsmith => "minecraft:toolsmith",
            Self::Weaponsmith => "minecraft:weaponsmith",
            Self::Home => "minecraft:home",
            Self::Meeting => "minecraft:meeting",
            Self::Beehive => "minecraft:beehive",
            Self::BeeNest => "minecraft:bee_nest",
            Self::NetherPortal => "minecraft:nether_portal",
            Self::Lodestone => "minecraft:lodestone",
            Self::TestInstance => "minecraft:test_instance",
            Self::LightningRod => "minecraft:lightning_rod",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|poi_type| poi_type.name() == name)
    }

    /// Vanilla `PoiType.maxTickets`, from `PoiTypes.bootstrap`.
    #[must_use]
    pub const fn max_tickets(self) -> i32 {
        match self {
            Self::Meeting => 32,
            Self::Beehive
            | Self::BeeNest
            | Self::NetherPortal
            | Self::Lodestone
            | Self::TestInstance
            | Self::LightningRod => 0,
            _ => 1,
        }
    }

    /// Vanilla `PoiType.validRange`, from `PoiTypes.bootstrap`.
    #[must_use]
    pub const fn valid_range(self) -> i32 {
        match self {
            Self::Meeting => 6,
            _ => 1,
        }
    }

    /// Whether this type is in the given `point_of_interest_type` tag.
    #[must_use]
    pub fn is_in(self, tag: &Tag) -> bool {
        tag.1.contains(&(self as u16))
    }

    #[must_use]
    pub fn is_village(self) -> bool {
        self.is_in(&tag::PointOfInterestType::MINECRAFT_VILLAGE)
    }

    /// Vanilla `PoiTypes.forState`.
    #[must_use]
    pub fn for_state(state: BlockStateId) -> Option<Self> {
        TYPE_BY_STATE
            .get(usize::from(state.as_u16()))
            .and_then(|&id| Self::ALL.get(usize::from(id)).copied())
    }

    fn compute_for_state(state: BlockStateId) -> Option<Self> {
        let block = state.to_block();
        Some(match block {
            b if b == &Block::BLAST_FURNACE => Self::Armorer,
            b if b == &Block::SMOKER => Self::Butcher,
            b if b == &Block::CARTOGRAPHY_TABLE => Self::Cartographer,
            b if b == &Block::BREWING_STAND => Self::Cleric,
            b if b == &Block::COMPOSTER => Self::Farmer,
            b if b == &Block::BARREL => Self::Fisherman,
            b if b == &Block::FLETCHING_TABLE => Self::Fletcher,
            b if b == &Block::CAULDRON
                || b == &Block::LAVA_CAULDRON
                || b == &Block::WATER_CAULDRON
                || b == &Block::POWDER_SNOW_CAULDRON =>
            {
                Self::Leatherworker
            }
            b if b == &Block::LECTERN => Self::Librarian,
            b if b == &Block::STONECUTTER => Self::Mason,
            b if b == &Block::LOOM => Self::Shepherd,
            b if b == &Block::SMITHING_TABLE => Self::Toolsmith,
            b if b == &Block::GRINDSTONE => Self::Weaponsmith,
            b if b.has_tag(&tag::Block::MINECRAFT_BEDS) => {
                if BedProperties::from_state_id(state).part != BedPart::Head {
                    return None;
                }
                Self::Home
            }
            b if b == &Block::BELL => Self::Meeting,
            b if b == &Block::BEEHIVE => Self::Beehive,
            b if b == &Block::BEE_NEST => Self::BeeNest,
            b if b == &Block::NETHER_PORTAL => Self::NetherPortal,
            b if b == &Block::LODESTONE => Self::Lodestone,
            b if b == &Block::TEST_INSTANCE_BLOCK => Self::TestInstance,
            b if b.has_tag(&tag::Block::MINECRAFT_LIGHTNING_RODS) => Self::LightningRod,
            _ => return None,
        })
    }
}

const NO_POI: u8 = u8::MAX;

/// Vanilla `PoiTypes.TYPE_BY_STATE`, flattened to one byte per block state.
static TYPE_BY_STATE: LazyLock<Box<[u8]>> = LazyLock::new(|| {
    (0..BlockStateId::COUNT)
        .map(|id| {
            BlockStateId::new(id)
                .and_then(PoiType::compute_for_state)
                .map_or(NO_POI, |poi_type| poi_type as u8)
        })
        .collect()
});

/// Vanilla `PoiManager.Occupancy`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Occupancy {
    HasSpace,
    IsOccupied,
    Any,
}

/// Vanilla `PoiRecord`. Tickets are atomic so claims work under the manager's read lock.
pub struct PoiRecord {
    pub pos: BlockPos,
    pub poi_type: PoiType,
    free_tickets: AtomicI32,
}

impl PoiRecord {
    const fn new(pos: BlockPos, poi_type: PoiType, free_tickets: i32) -> Self {
        Self {
            pos,
            poi_type,
            free_tickets: AtomicI32::new(free_tickets),
        }
    }

    #[must_use]
    pub fn free_tickets(&self) -> i32 {
        self.free_tickets.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn has_space(&self) -> bool {
        self.free_tickets() > 0
    }

    #[must_use]
    pub fn is_occupied(&self) -> bool {
        self.free_tickets() != self.poi_type.max_tickets()
    }

    fn matches(&self, occupancy: Occupancy) -> bool {
        match occupancy {
            Occupancy::HasSpace => self.has_space(),
            Occupancy::IsOccupied => self.is_occupied(),
            Occupancy::Any => true,
        }
    }

    /// Vanilla `acquireTicket`; false when another claimant got the last ticket first.
    fn acquire_ticket(&self) -> bool {
        self.free_tickets
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |tickets| {
                (tickets > 0).then(|| tickets - 1)
            })
            .is_ok()
    }

    /// Vanilla `releaseTicket`.
    fn release_ticket(&self) -> bool {
        let max = self.poi_type.max_tickets();
        self.free_tickets
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |tickets| {
                (tickets < max).then(|| tickets + 1)
            })
            .is_ok()
    }
}

/// Vanilla `PoiManager`, holding the POIs of loaded chunks.
///
/// A POI region file's coordinates.
type RegionPos = (i32, i32);
/// Region files read off the tick thread, handed back to the tick.
type LoadedRegions = Arc<std::sync::Mutex<Vec<(RegionPos, PoiRegion)>>>;

/// Nether portals stay with the portal code, which keeps its own entries in the same region
/// files. Everything else is indexed here and its tickets are written through to those files.
#[derive(Default)]
pub struct PoiManager {
    chunks: std::sync::RwLock<FxHashMap<Vector2<i32>, Vec<PoiRecord>>>,
    /// Bumped whenever which sections count as village centres could change.
    generation: AtomicU64,
    village_distance_cache: DashMap<(i32, i32, i32), (u64, i32)>,
    /// Chunks with POIs whose region file is still being read, by region.
    waiting_chunks: std::sync::Mutex<FxHashMap<RegionPos, Vec<Vector2<i32>>>>,
    /// Waiting for `apply_loaded_regions`.
    loaded_regions: LoadedRegions,
}

fn section_of(pos: &BlockPos) -> (i32, i32, i32) {
    (pos.0.x >> 4, pos.0.y >> 4, pos.0.z >> 4)
}

fn dist_sqr(a: &BlockPos, b: &BlockPos) -> i64 {
    let dx = i64::from(a.0.x - b.0.x);
    let dy = i64::from(a.0.y - b.0.y);
    let dz = i64::from(a.0.z - b.0.z);
    dx * dx + dy * dy + dz * dz
}

impl PoiManager {
    fn read_chunks(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, FxHashMap<Vector2<i32>, Vec<PoiRecord>>> {
        self.chunks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_chunks(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, FxHashMap<Vector2<i32>, Vec<PoiRecord>>> {
        self.chunks
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn bump_generation(&self, poi_type: PoiType) {
        if poi_type.is_village() {
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Vanilla `getInRange`: visits matching records within `radius`, stopping when `visit`
    /// returns true. Chunks go in vanilla's `ChunkPos.rangeClosed` order.
    fn visit_in_range(
        &self,
        predicate: impl Fn(PoiType) -> bool,
        center: &BlockPos,
        radius: i32,
        occupancy: Occupancy,
        mut visit: impl FnMut(&PoiRecord) -> bool,
    ) {
        let chunk_radius = radius.div_euclid(16) + 1;
        let center_chunk = Vector2::new(center.0.x >> 4, center.0.z >> 4);
        let radius_sqr = i64::from(radius) * i64::from(radius);
        let chunks = self.read_chunks();
        for chunk_z in center_chunk.y - chunk_radius..=center_chunk.y + chunk_radius {
            for chunk_x in center_chunk.x - chunk_radius..=center_chunk.x + chunk_radius {
                let Some(records) = chunks.get(&Vector2::new(chunk_x, chunk_z)) else {
                    continue;
                };
                for record in records {
                    if predicate(record.poi_type)
                        && record.matches(occupancy)
                        && (record.pos.0.x - center.0.x).abs() <= radius
                        && (record.pos.0.z - center.0.z).abs() <= radius
                        && dist_sqr(&record.pos, center) <= radius_sqr
                        && visit(record)
                    {
                        return;
                    }
                }
            }
        }
    }

    /// Vanilla `getCountInRange`.
    #[must_use]
    pub fn get_count_in_range(
        &self,
        predicate: impl Fn(PoiType) -> bool,
        center: &BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> usize {
        let mut count = 0;
        self.visit_in_range(predicate, center, radius, occupancy, |_| {
            count += 1;
            false
        });
        count
    }

    /// Vanilla `findAllWithType`. `filter` sees every candidate once, in search order, as the
    /// stateful filters vanilla passes here expect.
    #[must_use]
    pub fn find_all_with_type(
        &self,
        predicate: impl Fn(PoiType) -> bool,
        mut filter: impl FnMut(&BlockPos) -> bool,
        center: &BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> Vec<(PoiType, BlockPos)> {
        let mut found = Vec::new();
        self.visit_in_range(predicate, center, radius, occupancy, |record| {
            if filter(&record.pos) {
                found.push((record.poi_type, record.pos));
            }
            false
        });
        found
    }

    /// Vanilla `findAllClosestFirstWithType`.
    #[must_use]
    pub fn find_all_closest_first_with_type(
        &self,
        predicate: impl Fn(PoiType) -> bool,
        filter: impl FnMut(&BlockPos) -> bool,
        center: &BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> Vec<(PoiType, BlockPos)> {
        let mut found = self.find_all_with_type(predicate, filter, center, radius, occupancy);
        found.sort_by_key(|(_, pos)| dist_sqr(pos, center));
        found
    }

    /// Vanilla `findClosest` with a position filter.
    #[must_use]
    pub fn find_closest(
        &self,
        predicate: impl Fn(PoiType) -> bool,
        filter: impl Fn(&BlockPos) -> bool,
        center: &BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> Option<BlockPos> {
        self.find_closest_with_type(predicate, filter, center, radius, occupancy)
            .map(|(_, pos)| pos)
    }

    /// Vanilla `findClosestWithType`, with an optional position filter.
    #[must_use]
    pub fn find_closest_with_type(
        &self,
        predicate: impl Fn(PoiType) -> bool,
        filter: impl Fn(&BlockPos) -> bool,
        center: &BlockPos,
        radius: i32,
        occupancy: Occupancy,
    ) -> Option<(PoiType, BlockPos)> {
        let mut best: Option<(i64, PoiType, BlockPos)> = None;
        self.visit_in_range(predicate, center, radius, occupancy, |record| {
            let distance = dist_sqr(&record.pos, center);
            if filter(&record.pos) && best.is_none_or(|(closest, _, _)| distance < closest) {
                best = Some((distance, record.poi_type, record.pos));
            }
            false
        });
        best.map(|(_, poi_type, pos)| (poi_type, pos))
    }

    /// Vanilla `getRandom`: a random matching position that passes `filter`.
    #[must_use]
    pub fn get_random(
        &self,
        predicate: impl Fn(PoiType) -> bool,
        filter: impl Fn(&BlockPos) -> bool,
        occupancy: Occupancy,
        center: &BlockPos,
        radius: i32,
        rng: &mut impl rand::Rng,
    ) -> Option<BlockPos> {
        use rand::seq::SliceRandom;
        let mut found = Vec::new();
        self.visit_in_range(predicate, center, radius, occupancy, |record| {
            found.push(record.pos);
            false
        });
        found.shuffle(rng);
        found.into_iter().find(|pos| filter(pos))
    }

    /// Vanilla `exists`.
    #[must_use]
    pub fn exists(&self, pos: &BlockPos, predicate: impl Fn(PoiType) -> bool) -> bool {
        self.get_type(pos).is_some_and(predicate)
    }

    /// Vanilla `getType`.
    #[must_use]
    pub fn get_type(&self, pos: &BlockPos) -> Option<PoiType> {
        self.read_chunks()
            .get(&Vector2::new(pos.0.x >> 4, pos.0.z >> 4))?
            .iter()
            .find(|record| record.pos == *pos)
            .map(|record| record.poi_type)
    }

    fn persist_tickets(world: &World, record: &PoiRecord) {
        world
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_free_tickets(&record.pos, record.free_tickets());
    }

    /// Vanilla `take`: claims a ticket on the first free match that passes `filter`.
    pub fn take(
        &self,
        world: &World,
        predicate: impl Fn(PoiType) -> bool,
        filter: impl Fn(PoiType, &BlockPos) -> bool,
        center: &BlockPos,
        radius: i32,
    ) -> Option<BlockPos> {
        let mut taken = None;
        self.visit_in_range(predicate, center, radius, Occupancy::HasSpace, |record| {
            if filter(record.poi_type, &record.pos) && record.acquire_ticket() {
                Self::persist_tickets(world, record);
                taken = Some((record.poi_type, record.pos));
                return true;
            }
            false
        });
        let (poi_type, pos) = taken?;
        self.bump_generation(poi_type);
        Some(pos)
    }

    /// Vanilla `release`: gives back one ticket of the POI at `pos`.
    pub fn release(&self, world: &World, pos: &BlockPos) -> bool {
        let released = {
            let chunks = self.read_chunks();
            chunks
                .get(&Vector2::new(pos.0.x >> 4, pos.0.z >> 4))
                .and_then(|records| records.iter().find(|record| record.pos == *pos))
                .and_then(|record| {
                    record.release_ticket().then(|| {
                        Self::persist_tickets(world, record);
                        record.poi_type
                    })
                })
        };
        released.is_some_and(|poi_type| {
            self.bump_generation(poi_type);
            true
        })
    }

    /// Vanilla `sectionsToVillage`: sections to the nearest one holding a claimed village POI,
    /// or `MAX_VILLAGE_DISTANCE + 1` when there is none that close.
    #[must_use]
    pub fn sections_to_village(&self, section: (i32, i32, i32)) -> i32 {
        let generation = self.generation.load(Ordering::Relaxed);
        if let Some(cached) = self.village_distance_cache.get(&section)
            && cached.0 == generation
        {
            return cached.1;
        }
        let mut level = NOT_A_VILLAGE;
        {
            let chunks = self.read_chunks();
            let (sx, sy, sz) = section;
            for chunk_z in sz - MAX_VILLAGE_DISTANCE..=sz + MAX_VILLAGE_DISTANCE {
                for chunk_x in sx - MAX_VILLAGE_DISTANCE..=sx + MAX_VILLAGE_DISTANCE {
                    let Some(records) = chunks.get(&Vector2::new(chunk_x, chunk_z)) else {
                        continue;
                    };
                    for record in records {
                        if !record.poi_type.is_village() || !record.is_occupied() {
                            continue;
                        }
                        let (rx, ry, rz) = section_of(&record.pos);
                        let distance = (rx - sx).abs().max((ry - sy).abs()).max((rz - sz).abs());
                        level = level.min(distance);
                    }
                }
            }
        }
        self.village_distance_cache
            .insert(section, (generation, level));
        level
    }

    /// Indexes a chunk that just loaded: the POIs its blocks hold, with the ticket counts saved
    /// for them. If their region file isn't in memory yet, it is read off the tick thread and the
    /// chunk is indexed once `apply_loaded_regions` picks it up.
    pub fn on_chunk_loaded(&self, world: &World, chunk: Vector2<i32>) {
        let Some(found) = Self::scan_chunk(world, chunk) else {
            return;
        };
        if found.is_empty() {
            self.write_chunks().remove(&chunk);
            return;
        }
        let region = PortalPoiStorage::chunk_region(chunk.x, chunk.y);
        let mut storage = world
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !storage.is_region_loaded(region) {
            let path = storage.region_file(region);
            drop(storage);
            self.queue_region_load(world, region, chunk, path);
            return;
        }
        let records = Self::build_records(&mut storage, chunk, found);
        drop(storage);
        self.write_chunks().insert(chunk, records);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Parks a chunk until its region is read, starting that read on the blocking pool if it's
    /// the first chunk to wait for it. Reading and inflating a region file on the tick would stall
    /// every player, and villages are where these load.
    fn queue_region_load(
        &self,
        world: &World,
        region: RegionPos,
        chunk: Vector2<i32>,
        path: std::path::PathBuf,
    ) {
        let first = {
            let mut waiting = self
                .waiting_chunks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let chunks = waiting.entry(region).or_default();
            chunks.push(chunk);
            chunks.len() == 1
        };
        if !first {
            return;
        }
        let loaded_regions = Arc::clone(&self.loaded_regions);
        let load = move || {
            let loaded = PoiRegion::load_or_empty(&path);
            loaded_regions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((region, loaded));
        };
        match world.server.upgrade() {
            Some(server) => drop(server.runtime.spawn_blocking(load)),
            None => load(),
        }
    }

    /// Adds region files read off the tick thread and indexes the chunks that waited for them.
    pub fn apply_loaded_regions(&self, world: &World) {
        let loaded = std::mem::take(
            &mut *self
                .loaded_regions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if loaded.is_empty() {
            return;
        }
        let mut storage = world
            .portal_poi
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (region, region_data) in loaded {
            storage.insert_region(region, region_data);
            let chunks = self
                .waiting_chunks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&region)
                .unwrap_or_default();
            for chunk in chunks {
                // Rescanned, so blocks changed while the file loaded are counted.
                if !world.level.is_chunk_loaded(&chunk) {
                    continue;
                }
                let Some(found) = Self::scan_chunk(world, chunk) else {
                    continue;
                };
                if found.is_empty() {
                    continue;
                }
                let records = Self::build_records(&mut storage, chunk, found);
                self.write_chunks().insert(chunk, records);
            }
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The POIs a loaded chunk's blocks hold. Vanilla `checkConsistencyWithBlocks`, which only
    /// scans sections whose palette can hold a POI.
    fn scan_chunk(world: &World, chunk: Vector2<i32>) -> Option<Vec<(BlockPos, PoiType)>> {
        world.level.read_chunk_sync(&chunk, |chunk_data| {
            let sections = chunk_data
                .section
                .block_sections
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let min_y = chunk_data.section.min_y;
            let mut found = Vec::new();
            for (index, section) in sections.iter().enumerate() {
                if !section.any(|state| {
                    PoiType::for_state(state).is_some_and(|t| t != PoiType::NetherPortal)
                }) {
                    continue;
                }
                let base_y = min_y + (index as i32) * 16;
                for (offset, state) in section.iter().enumerate() {
                    let Some(poi_type) = PoiType::for_state(state) else {
                        continue;
                    };
                    if poi_type == PoiType::NetherPortal {
                        continue;
                    }
                    let (y, z, x) = (offset / 256, (offset / 16) % 16, offset % 16);
                    found.push((
                        BlockPos::new(
                            (chunk.x << 4) + x as i32,
                            base_y + y as i32,
                            (chunk.y << 4) + z as i32,
                        ),
                        poi_type,
                    ));
                }
            }
            found
        })
    }

    /// Pairs the POIs found in a chunk with the ticket counts saved for them, storing any the
    /// region file didn't have yet. The chunk's region must already be loaded.
    fn build_records(
        storage: &mut PortalPoiStorage,
        chunk: Vector2<i32>,
        found: Vec<(BlockPos, PoiType)>,
    ) -> Vec<PoiRecord> {
        let stored: FxHashMap<BlockPos, (Option<PoiType>, i32)> = storage
            .chunk_entries(chunk.x, chunk.y)
            .into_iter()
            .map(|entry| {
                (
                    entry.pos(),
                    (PoiType::from_name(&entry.poi_type), entry.free_tickets),
                )
            })
            .collect();
        found
            .into_iter()
            .map(|(pos, poi_type)| {
                let tickets = match stored.get(&pos) {
                    Some((Some(stored_type), tickets)) if *stored_type == poi_type => {
                        (*tickets).clamp(0, poi_type.max_tickets())
                    }
                    _ => {
                        storage.add_with_free_tickets(pos, poi_type.name(), poi_type.max_tickets());
                        poi_type.max_tickets()
                    }
                };
                PoiRecord::new(pos, poi_type, tickets)
            })
            .collect()
    }

    pub fn on_chunk_unloaded(&self, chunk: Vector2<i32>) {
        if self.write_chunks().remove(&chunk).is_some() {
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Keeps the index in step with a block change, as vanilla `ServerLevel.onBlockStateChange`.
    pub fn on_block_changed(
        &self,
        world: &World,
        pos: &BlockPos,
        old_state: BlockStateId,
        new_state: BlockStateId,
    ) {
        let old_type = PoiType::for_state(old_state).filter(|t| *t != PoiType::NetherPortal);
        let new_type = PoiType::for_state(new_state).filter(|t| *t != PoiType::NetherPortal);
        if old_type == new_type {
            return;
        }
        let chunk = Vector2::new(pos.0.x >> 4, pos.0.z >> 4);
        {
            let mut storage = world
                .portal_poi
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if old_type.is_some() {
                storage.remove(pos);
            }
            if let Some(new_type) = new_type {
                storage.add_with_free_tickets(*pos, new_type.name(), new_type.max_tickets());
            }
        }
        let mut chunks = self.write_chunks();
        if old_type.is_some()
            && let Some(records) = chunks.get_mut(&chunk)
        {
            records.retain(|record| record.pos != *pos);
        }
        if let Some(new_type) = new_type {
            chunks.entry(chunk).or_default().push(PoiRecord::new(
                *pos,
                new_type,
                new_type.max_tickets(),
            ));
        }
        drop(chunks);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}
