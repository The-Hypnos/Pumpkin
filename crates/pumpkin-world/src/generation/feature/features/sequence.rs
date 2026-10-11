use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::feature::placed_features::PlacedFeatureWrapper, world::WorldPortalExt};

pub struct SequenceFeature {
    pub features: Vec<PlacedFeatureWrapper>,
}

impl SequenceFeature {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        self.features.iter().all(|feature| {
            feature.get().is_some_and(|feature| {
                feature.generate(
                    chunk,
                    block_registry,
                    min_y,
                    height,
                    feature_name,
                    random,
                    pos,
                )
            })
        })
    }
}
