pub mod aquifer_sampler;
pub mod perlin;
pub mod router;

use pumpkin_data::{Block, BlockState, noise_settings::GenerationShapeConfig};
use pumpkin_util::math::vector3::Vector3;

use crate::generation::{
    noise::aquifer_sampler::{
        AquiferSampler, AquiferSamplerImpl, SeaLevelAquiferSampler, WorldAquiferSampler,
    },
    proto_chunk::StandardChunkFluidLevelSampler,
    section_coords,
};

use super::{
    GlobalRandomConfig,
    noise::router::{
        chunk_density_function::ChunkNoiseFunctionBuilderOptions,
        chunk_noise_router::ChunkNoiseRouter,
        density_volume::{DensityBuffer, DensityVolume},
        proto_noise_router::ProtoNoiseRouter,
        surface_height_sampler::SurfaceHeightEstimateSampler,
    },
};

pub const LAVA_BLOCK: Block = Block::LAVA;
pub const WATER_BLOCK: Block = Block::WATER;

pub const CHUNK_DIM: u8 = 16;

pub struct ChunkDensities {
    pub density: DensityBuffer,
}

pub struct ChunkNoiseGenerator<'a> {
    pub aquifer: AquiferSampler,
    generation_shape: &'a GenerationShapeConfig,
    volume: DensityVolume,
    pub router: ChunkNoiseRouter<'a>,
}

impl<'a> ChunkNoiseGenerator<'a> {
    #[expect(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        noise_router_base: &'a ProtoNoiseRouter,
        random_config: &GlobalRandomConfig,
        volume: DensityVolume,
        generation_shape: &'a GenerationShapeConfig,
        level_sampler: StandardChunkFluidLevelSampler,
        aquifers: bool,
        beardifier_structures: Vec<
            crate::generation::noise::router::density_function::beardifier::BeardifierStructure,
        >,
        beardifier_junctions: Vec<
            crate::generation::noise::router::density_function::beardifier::BeardifierJunction,
        >,
        affected_box: Option<pumpkin_util::math::block_box::BlockBox>,
    ) -> Self {
        let builder_options = ChunkNoiseFunctionBuilderOptions::new(
            beardifier_structures,
            beardifier_junctions,
            affected_box,
        );

        let aquifer_sampler = if aquifers {
            let section_x = section_coords::block_to_section(volume.min_block_x);
            let section_z = section_coords::block_to_section(volume.min_block_z);
            AquiferSampler::Aquifer(WorldAquiferSampler::new(
                section_x,
                section_z,
                &random_config.aquifer_random_deriver,
                generation_shape.min_y,
                generation_shape.height,
                level_sampler,
            ))
        } else {
            AquiferSampler::SeaLevel(SeaLevelAquiferSampler::new(level_sampler))
        };

        let router = ChunkNoiseRouter::generate(noise_router_base, &builder_options);

        Self {
            aquifer: aquifer_sampler,
            generation_shape,
            volume,
            router,
        }
    }

    #[must_use]
    pub const fn volume(&self) -> &DensityVolume {
        &self.volume
    }

    pub fn sample_density(&mut self) -> ChunkDensities {
        let mut density = DensityBuffer::acquire(&self.volume);
        self.router.final_density_volume(&mut density, &self.volume);
        ChunkDensities { density }
    }

    /// Vanilla `Aquifer.computeSubstance`; `None` means the default block.
    pub fn sample_block_state(
        &mut self,
        pos: &Vector3<i32>,
        density: f32,
        height_estimator: &mut SurfaceHeightEstimateSampler,
    ) -> Option<&'static BlockState> {
        self.aquifer
            .apply(&mut self.router, pos, density, height_estimator)
            .0
    }

    #[inline]
    #[must_use]
    pub const fn min_y(&self) -> i8 {
        self.generation_shape.min_y
    }

    #[inline]
    #[must_use]
    pub const fn height(&self) -> u16 {
        self.generation_shape.height
    }
}
