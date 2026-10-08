use std::sync::atomic::AtomicBool;

use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::projectile::{ProjectileHit, ThrownItemEntity};
use crate::{
    entity::{Entity, EntityBase},
    server::Server,
};
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

const GRAVITY: f64 = 0.07;
/// Splash particle colour vanilla passes with `PARTICLES_SPELL_POTION_SPLASH`.
const SPLASH_COLOR: i32 = -13_083_194;

/// Vanilla `ThrownExperienceBottle`.
pub struct ExperienceBottleEntity {
    pub thrown: ThrownItemEntity,
}

impl ExperienceBottleEntity {
    pub const fn new(entity: Entity) -> Self {
        Self {
            thrown: ThrownItemEntity {
                entity,
                owner_id: None,
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: GRAVITY,
            },
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        Self {
            thrown: ThrownItemEntity::new(entity, shooter, GRAVITY),
        }
    }
}

impl EntityBase for ExperienceBottleEntity {
    fn get_owner_id(&self) -> Option<i32> {
        self.thrown.owner_id
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown.process_tick(caller);
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let block_pos = BlockPos::floored_v(entity.pos.load());
        world.sync_world_event(
            WorldEvent::ParticlesSpellPotionSplash,
            block_pos,
            SPLASH_COLOR,
        );
        if !entity.silent.load(std::sync::atomic::Ordering::Relaxed) {
            world.sync_world_event(WorldEvent::SoundSpellPotionSplash, block_pos, 0);
        }

        let mut rng = rand::rng();
        let xp = 3 + rng.random_range(0..5) + rng.random_range(0..5);
        let direction = match &hit {
            ProjectileHit::Block { face, .. } => face.to_offset().to_f64(),
            ProjectileHit::Entity { .. } => entity.velocity.load() * -1.0,
        };
        ExperienceOrbEntity::award_with_direction(&world, hit.hit_pos(), direction, xp);
    }
}
