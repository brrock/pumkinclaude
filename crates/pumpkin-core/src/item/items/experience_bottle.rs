use std::any::Any;
use std::sync::Arc;

use crate::entity::player::Player;
use crate::entity::projectile::experience_bottle::ExperienceBottleEntity;
use crate::entity::{Entity, EntityBase};
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::Hand;
use rand::{RngExt, rng};

pub struct ExperienceBottleItem;

impl ItemMetadata for ExperienceBottleItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::EXPERIENCE_BOTTLE.id])
    }
}

const ANGLE_OFFSET: f32 = -20.0;
const POWER: f32 = 0.7;
const UNCERTAINTY: f32 = 1.0;

impl ItemBehaviour for ExperienceBottleItem {
    fn normal_use_with_hand(
        &self,
        _item: &Item,
        player: &Player,
        yaw: f32,
        pitch: f32,
        hand: Hand,
    ) {
        let world = player.world();
        let position = player.position();
        world.play_sound_fine(
            Sound::EntityExperienceBottleThrow,
            SoundCategory::Neutral,
            &position,
            0.5,
            0.4 / (rng().random::<f32>() * 0.4 + 0.8),
        );

        let entity = Entity::new(world.clone(), position, &EntityType::EXPERIENCE_BOTTLE);
        let bottle = ExperienceBottleEntity::new_shot(entity, player.get_entity());
        bottle
            .thrown
            .set_velocity_from(pitch, yaw, ANGLE_OFFSET, POWER, UNCERTAINTY);
        world.spawn_entity(Arc::new(bottle));

        let mut stack = player.inventory.get_stack_in_hand(hand);
        stack.decrement_unless_creative(player.gamemode.load(), 1);
        player.inventory.set_stack_in_hand(hand, stack);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
