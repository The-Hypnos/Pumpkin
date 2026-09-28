//! Vanilla `GossipContainer`: what a villager has heard about players and mobs.

use rand::RngExt;
use rustc_hash::FxHashMap;
use uuid::Uuid;

use super::data::GossipType;

/// Vanilla `ReputationEventType`: what a villager saw happen that changes its gossip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReputationEventType {
    ZombieVillagerCured,
    Trade,
    VillagerHurt,
    VillagerKilled,
}

/// Vanilla `GossipContainer.DISCARD_THRESHOLD`.
const DISCARD_THRESHOLD: i32 = 2;

#[derive(Default, Clone)]
pub struct GossipContainer {
    gossips: FxHashMap<Uuid, FxHashMap<GossipType, i32>>,
}

/// One `(target, type, value)` rumour, vanilla `GossipEntry`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GossipEntry {
    pub target: Uuid,
    pub gossip_type: GossipType,
    pub value: i32,
}

impl GossipEntry {
    const fn weighted_value(&self) -> i32 {
        self.value * self.gossip_type.weight()
    }
}

impl GossipContainer {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.gossips.is_empty()
    }

    /// Every rumour, as vanilla `unpack` streams them.
    pub fn entries(&self) -> impl Iterator<Item = GossipEntry> + '_ {
        self.gossips.iter().flat_map(|(target, entries)| {
            entries.iter().map(|(gossip_type, value)| GossipEntry {
                target: *target,
                gossip_type: *gossip_type,
                value: *value,
            })
        })
    }

    /// Restores one saved rumour without the clamping `add` does, as vanilla's codec.
    pub fn put(&mut self, entry: GossipEntry) {
        self.gossips
            .entry(entry.target)
            .or_default()
            .insert(entry.gossip_type, entry.value);
    }

    pub fn clear(&mut self) {
        self.gossips.clear();
    }

    /// Vanilla `decay`, run once a day.
    pub fn decay(&mut self) {
        self.gossips.retain(|_, entries| {
            entries.retain(|gossip_type, value| {
                *value -= gossip_type.daily_decay();
                *value >= DISCARD_THRESHOLD
            });
            !entries.is_empty()
        });
    }

    /// Vanilla `selectGossipsForTransfer`: up to `max_count` rumours, picked by weight.
    fn select_gossips_for_transfer(
        &self,
        rng: &mut impl rand::Rng,
        max_count: usize,
    ) -> Vec<GossipEntry> {
        let entries: Vec<GossipEntry> = self.entries().collect();
        if entries.is_empty() {
            return Vec::new();
        }
        let mut ranges = Vec::with_capacity(entries.len());
        let mut ranges_end = 0;
        for entry in &entries {
            ranges_end += entry.weighted_value().abs();
            ranges.push(ranges_end - 1);
        }
        let mut chosen: Vec<GossipEntry> = Vec::new();
        for _ in 0..max_count {
            let choice = rng.random_range(0..ranges_end);
            let index = ranges.partition_point(|end| *end < choice);
            let entry = entries[index];
            if !chosen.contains(&entry) {
                chosen.push(entry);
            }
        }
        chosen
    }

    /// Vanilla `transferFrom`: hears up to `max_count` rumours from `source`, a little weaker
    /// than `source` knows them. Returns how many were picked.
    pub fn transfer_from(
        &mut self,
        source: &Self,
        rng: &mut impl rand::Rng,
        max_count: usize,
    ) -> usize {
        let new_gossips = source.select_gossips_for_transfer(rng, max_count);
        for gossip in &new_gossips {
            let decayed = gossip.value - gossip.gossip_type.decay_per_transfer();
            if decayed >= DISCARD_THRESHOLD {
                let value = self
                    .gossips
                    .entry(gossip.target)
                    .or_default()
                    .entry(gossip.gossip_type)
                    .or_insert(decayed);
                *value = (*value).max(decayed);
            }
        }
        new_gossips.len()
    }

    /// Vanilla `getReputation`.
    #[must_use]
    pub fn get_reputation(&self, target: &Uuid, types: impl Fn(GossipType) -> bool) -> i32 {
        self.gossips.get(target).map_or(0, |entries| {
            entries
                .iter()
                .filter(|(gossip_type, _)| types(**gossip_type))
                .map(|(gossip_type, value)| value * gossip_type.weight())
                .sum()
        })
    }

    /// Vanilla `add`.
    pub fn add(&mut self, target: Uuid, gossip_type: GossipType, amount: i32) {
        let entries = self.gossips.entry(target).or_default();
        let max = gossip_type.max_value();
        let value = entries.entry(gossip_type).or_insert(0);
        let old = *value;
        let sum = old + amount;
        *value = if sum > max { max.max(old) } else { sum };
        let value = *value;
        if value > max {
            entries.insert(gossip_type, max);
        }
        if value < DISCARD_THRESHOLD {
            entries.remove(&gossip_type);
        }
        if entries.is_empty() {
            self.gossips.remove(&target);
        }
    }
}
