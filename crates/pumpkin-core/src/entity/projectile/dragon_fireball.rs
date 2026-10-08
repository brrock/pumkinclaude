use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::area_effect_cloud::{AreaEffectCloudEntity, CloudParticle};
use crate::entity::projectile::fireball::{INITIAL_ACCELERATION_POWER, apply_inertia};
use crate::entity::projectile::{ProjectileHit, ThrownItemEntity};
use crate::{
    entity::{Entity, EntityBase},
    server::Server,
};

/// Vanilla `DragonFireball.SPLASH_RANGE`.
pub const SPLASH_RANGE: f64 = 4.0;
const CLOUD_RADIUS: f32 = 3.0;
const CLOUD_MAX_RADIUS: f32 = 7.0;
const CLOUD_DURATION: i32 = 600;
const CLOUD_POTION_DURATION_SCALE: f32 = 0.25;

/// Vanilla `DragonFireball`.
pub struct DragonFireballEntity {
    pub thrown: ThrownItemEntity,
}

impl DragonFireballEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        Self {
            thrown: ThrownItemEntity {
                entity,
                owner_id: None,
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: 0.0,
            },
        }
    }

    /// Fired by `owner` from `position` along `direction`.
    #[must_use]
    pub fn new_shot(
        entity: Entity,
        owner: &Entity,
        position: Vector3<f64>,
        direction: Vector3<f64>,
    ) -> Self {
        let mut fireball = Self::new(entity);
        fireball.thrown.owner_id = Some(owner.entity_id);
        fireball.thrown.entity.set_pos(position);
        let accel = INITIAL_ACCELERATION_POWER;
        fireball
            .thrown
            .entity
            .velocity
            .store(direction.normalize().multiply(accel, accel, accel));
        fireball
    }
}

impl EntityBase for DragonFireballEntity {
    fn get_owner_id(&self) -> Option<i32> {
        self.thrown.owner_id
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        apply_inertia(self.get_entity(), INITIAL_ACCELERATION_POWER);
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
        if let ProjectileHit::Entity { entity, .. } = &hit
            && Some(entity.get_entity().entity_id) == self.thrown.owner_id
        {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();

        let mut cloud = AreaEffectCloudEntity::new(Entity::new(
            world.clone(),
            pos,
            &EntityType::AREA_EFFECT_CLOUD,
        ));
        let settings = cloud.settings_mut();
        settings.owner_id = self.thrown.owner_id;
        settings.custom_particle = Some(CloudParticle::DragonBreath { power: 1.0 });
        settings.radius = CLOUD_RADIUS;
        settings.duration = CLOUD_DURATION;
        settings.radius_per_tick = (CLOUD_MAX_RADIUS - CLOUD_RADIUS) / CLOUD_DURATION as f32;
        settings.potion_duration_scale = CLOUD_POTION_DURATION_SCALE;
        settings
            .effects
            .push((&StatusEffect::INSTANT_DAMAGE, 1, 1, false, true, true));

        let splash_box = entity
            .bounding_box
            .load()
            .expand(SPLASH_RANGE, 2.0, SPLASH_RANGE);
        let nearby = world.get_entities_at_box(&splash_box).into_iter().chain(
            world
                .get_players_at_box(&splash_box)
                .into_iter()
                .filter(|player| !player.is_spectator())
                .map(|player| player as Arc<dyn EntityBase>),
        );
        for target in nearby {
            if target.get_living_entity().is_none() {
                continue;
            }
            let target_pos = target.get_entity().pos.load();
            if target_pos.squared_distance_to_vec(&pos) < SPLASH_RANGE * SPLASH_RANGE {
                cloud.entity.set_pos(target_pos);
                break;
            }
        }

        let silent = entity.silent.load(Ordering::Relaxed);
        world.sync_world_event(
            WorldEvent::ParticlesDragonFireballSplash,
            BlockPos::floored_v(pos),
            if silent { -1 } else { 1 },
        );
        world.spawn_entity(Arc::new(cloud));
    }
}
