use pumpkin_data::{BlockState, material_rule::OreVeinMaterialRule};
use pumpkin_util::{
    math::vector3::Vector3,
    random::{RandomDeriver, RandomDeriverImpl, RandomImpl},
};

use crate::generation::noise::router::{
    chunk_density_function::ChunkNoiseFunctionBuilderOptions,
    chunk_noise_router::ChunkNoiseRouter,
    density_volume::{DensityBuffer, DensityVolume},
    proto_noise_router::ProtoNoiseRouter,
};

/// Density function sources an ore vein rule reads, resolved once per rule.
#[derive(Clone, Copy)]
struct CompiledVein {
    density: usize,
    richness: usize,
    filler_gap: usize,
}

/// Evaluates vanilla `OreVeinRule`s for one surface pass, mirroring the samplers
/// `MaterialRuleContext.getDensitiesInChunk` hands out.
pub struct OreVeinSampler<'a> {
    noise_router: &'a ProtoNoiseRouter,
    random: &'a RandomDeriver,
    /// The chunk volume `density` and `richness` are prefilled over. Without one every
    /// sample is taken at its position, like vanilla's single-block `topMaterial` volume.
    volume: Option<DensityVolume>,
    router: Option<ChunkNoiseRouter<'a>>,
    /// Prefilled buffers keyed by router component index.
    buffers: Vec<(usize, DensityBuffer)>,
    compiled: Vec<(*const OreVeinMaterialRule, CompiledVein)>,
}

impl<'a> OreVeinSampler<'a> {
    #[must_use]
    pub const fn new(
        noise_router: &'a ProtoNoiseRouter,
        random: &'a RandomDeriver,
        volume: Option<DensityVolume>,
    ) -> Self {
        Self {
            noise_router,
            random,
            volume,
            router: None,
            buffers: Vec::new(),
            compiled: Vec::new(),
        }
    }

    #[expect(
        clippy::panic,
        reason = "codegen compiles every density function the material rule references into the router"
    )]
    fn component(&self, name: &str) -> usize {
        self.noise_router
            .material_functions
            .iter()
            .find(|(id, _)| *id == name)
            .map_or_else(
                || panic!("noise router is missing material function {name}"),
                |&(_, index)| index,
            )
    }

    fn router(&mut self) -> &mut ChunkNoiseRouter<'a> {
        let noise_router = self.noise_router;
        self.router.get_or_insert_with(|| {
            let options = ChunkNoiseFunctionBuilderOptions::new(Vec::new(), Vec::new(), None);
            ChunkNoiseRouter::generate(noise_router, &options)
        })
    }

    fn prefill(&mut self, component: usize) {
        let Some(volume) = self.volume else {
            return;
        };
        if self.buffers.iter().any(|(index, _)| *index == component) {
            return;
        }
        let mut buffer = DensityBuffer::acquire(&volume);
        self.router()
            .sample_component_volume(component, &mut buffer, &volume);
        self.buffers.push((component, buffer));
    }

    fn compile(&mut self, rule: &OreVeinMaterialRule) -> CompiledVein {
        let key: *const OreVeinMaterialRule = rule;
        if let Some(&(_, compiled)) = self.compiled.iter().find(|(ptr, _)| *ptr == key) {
            return compiled;
        }
        let compiled = CompiledVein {
            density: self.component(rule.density),
            richness: self.component(rule.richness),
            filler_gap: self.component(rule.filler_gap),
        };
        self.prefill(compiled.density);
        self.prefill(compiled.richness);
        self.compiled.push((key, compiled));
        compiled
    }

    fn sample(&mut self, component: usize, pos: &Vector3<i32>) -> f32 {
        if let Some(volume) = &self.volume
            && let Some(index) = volume.index_of_block(pos.x, pos.y, pos.z)
            && let Some((_, buffer)) = self.buffers.iter().find(|(c, _)| *c == component)
        {
            return buffer[index];
        }
        self.router().sample_component(component, pos)
    }

    /// Vanilla `OreVeinRule.compile`'s evaluator.
    pub fn try_apply(
        &mut self,
        rule: &OreVeinMaterialRule,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<&'static BlockState> {
        let compiled = self.compile(rule);
        let pos = Vector3::new(x, y, z);
        let density = self.sample(compiled.density, &pos);
        if density <= 0.0 {
            return None;
        }
        let mut random = self.random.split_pos(x, y, z);
        if random.next_f32() > density {
            return None;
        }
        let richness = self.sample(compiled.richness, &pos);
        if random.next_f32() < richness
            && self.router().sample_component(compiled.filler_gap, &pos) < 0.0
        {
            Some(if random.next_f32() < rule.raw_ore_chance {
                rule.raw_ore_block
            } else {
                rule.ore_block
            })
        } else {
            Some(rule.filler_block)
        }
    }
}
