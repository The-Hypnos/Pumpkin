use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::feature::placed_features::PlacedFeatureWrapper, world::WorldPortalExt};

pub struct OverlayFeature {
    pub features: Vec<PlacedFeatureWrapper>,
}

impl OverlayFeature {
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
        let mut placed_any = false;
        for feature in &self.features {
            if let Some(feature) = feature.get() {
                placed_any |= feature.generate(
                    chunk,
                    block_registry,
                    min_y,
                    height,
                    feature_name,
                    random,
                    pos,
                );
            }
        }
        placed_any
    }
}
