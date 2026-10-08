use std::sync::{Arc, RwLock};

use crate::entity::area_effect_cloud::AreaEffectCloudEntity;
use crate::item::potion::{PotionContents, is_instantaneous};
use std::sync::atomic::AtomicBool;

use crate::entity::projectile::splash_potion::extinguish_fire_if_water_potion;
use crate::{
    entity::{Entity, EntityBase, projectile::ThrownItemEntity},
    server::Server,
};
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::java::client::play::CWorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::{Vector2, to_chunk_pos};
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

const GRAVITY: f64 = 0.05;

pub struct LingeringPotionEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
}

impl LingeringPotionEntity {
    pub fn new(entity: Entity) -> Self {
        entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(
                1,
                &pumpkin_data::item::Item::LINGERING_POTION,
            )),
        }
    }

    pub fn new_shot(entity: Entity, shooter: &Entity) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        thrown.entity.set_velocity(Vector3::new(0.0, 0.1, 0.0));
        Self {
            thrown,
            item_stack: RwLock::new(ItemStack::new(
                1,
                &pumpkin_data::item::Item::LINGERING_POTION,
            )),
        }
    }

    pub fn set_item_stack(&self, item_stack: ItemStack) {
        let mut write = self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *write = item_stack;
    }
}

impl EntityBase for LingeringPotionEntity {
    fn get_owner_id(&self) -> Option<i32> {
        self.thrown.owner_id
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Sync the item stack so the client renders the correct potion type
        entity.set_synced_data(
            pumpkin_data::tracked_data::lingering_potion::ITEM_STACK,
            pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer::from(stack.clone()),
        );
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

    fn on_hit(&self, hit: crate::entity::projectile::ProjectileHit) {
        let world = self.get_entity().world.load();
        let hit_pos = hit.hit_pos();

        // Read stored item stack and compute potion effects
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        // Play impact particles
        world.send_entity_status(
            self.get_entity(),
            EntityStatus::Death,
            Some(ActorEventID::Death),
        );

        let effects = crate::item::potion::PotionContents::read_potion_effects(&stack);

        if effects.is_empty() {
            extinguish_fire_if_water_potion(&world, hit_pos, &stack);
        } else {
            if let Some(server) = world.server.upgrade() {
                let mut event = crate::plugin::api::events::entity::lingering_potion_splash::LingeringPotionSplashEvent::new(
                    self.get_entity().entity_id,
                    BlockPos::floored_v(hit_pos),
                    stack.item.registry_key.to_string(),
                );
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return;
                }
            }
            let pos = self.get_entity().pos.load();
            let mut cloud = AreaEffectCloudEntity::new(Entity::from_uuid(
                Uuid::new_v4(),
                world.clone(),
                pos,
                &pumpkin_data::entity::EntityType::AREA_EFFECT_CLOUD,
            ));
            // Vanilla `ThrownLingeringPotion.onHitAsPotion`.
            let settings = cloud.settings_mut();
            settings.owner_id = self.thrown.owner_id;
            settings.radius = 3.0;
            settings.radius_on_use = -0.5;
            settings.duration = 600;
            settings.wait_time = 10;
            settings.radius_per_tick = -settings.radius / settings.duration as f32;
            settings.potion = stack.clone();
            settings.effects.clone_from(&effects);
            world.spawn_entity(Arc::new(cloud));
        }

        let has_instant = effects.iter().any(|(e, ..)| is_instantaneous(e));
        let event_id = if has_instant { 2007 } else { 2002 };
        let block_pos = BlockPos::floored_v(self.get_entity().pos.load());
        let color = PotionContents::color_of(&stack, &effects);
        let chunk_pos = to_chunk_pos(&Vector2::new(block_pos.0.x, block_pos.0.z));
        world.broadcast_to_chunk(
            chunk_pos,
            &CWorldEvent::new(event_id, block_pos, color, false),
        );
    }
}
