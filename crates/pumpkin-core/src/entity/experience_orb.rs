use core::f32;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, AtomicU32, Ordering},
};

use pumpkin_data::{damage::DamageType, entity::EntityType, fluid::Fluid};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::{boundingbox::BoundingBox, vector3::Vector3};
use rand::RngExt;

use crate::{server::Server, world::World};

use super::{Entity, EntityBase, living::LivingEntity, player::Player};

const LIFETIME: i32 = 6000;
const MERGE_ID_SPREAD: i32 = 40;
const FOLLOW_DISTANCE: f64 = 8.0;

/// Vanilla `ExperienceOrb`.
pub struct ExperienceOrbEntity {
    entity: Entity,
    value: AtomicU32,
    count: AtomicU32,
    orb_age: AtomicI32,
    health: AtomicI32,
    following_player: Mutex<Option<Arc<Player>>>,
    /// Held while this orb's count changes, taken in entity id order when two orbs merge,
    /// because entities tick in parallel.
    merge_lock: Mutex<()>,
}

impl ExperienceOrbEntity {
    pub fn new(entity: Entity, value: u32) -> Self {
        entity.yaw.store(rand::random::<f32>() * 360.0);
        Self {
            entity,
            value: AtomicU32::new(value),
            count: AtomicU32::new(1),
            orb_age: AtomicI32::new(0),
            health: AtomicI32::new(5),
            following_player: Mutex::new(None),
            merge_lock: Mutex::new(()),
        }
    }

    /// Vanilla `ExperienceOrb.award`.
    pub fn award(world: &Arc<World>, position: Vector3<f64>, amount: u32) {
        Self::award_with_direction(world, position, Vector3::new(0.0, 0.0, 0.0), amount);
    }

    /// Vanilla `ExperienceOrb.awardWithDirection`.
    pub fn award_with_direction(
        world: &Arc<World>,
        position: Vector3<f64>,
        rough_direction: Vector3<f64>,
        mut amount: u32,
    ) {
        while amount > 0 {
            let value = Self::get_experience_value(amount);
            amount -= value;
            if !Self::try_merge_to_existing(world, position, value) {
                let entity = Entity::new(world.clone(), position, &EntityType::EXPERIENCE_ORB);
                let orb = Self::new(entity, value);
                orb.set_spawn_motion(position, rough_direction);
                world.spawn_entity(Arc::new(orb));
            }
        }
    }

    /// The random kick a new orb gets, turned away from `rough_direction`'s opposite side.
    fn set_spawn_motion(&self, position: Vector3<f64>, rough_direction: Vector3<f64>) {
        let mut rng = rand::rng();
        let mut motion = Vector3::new(
            (rng.random::<f64>() * 0.2 - 0.1) * 2.0,
            rng.random::<f64>() * 0.2 * 2.0,
            (rng.random::<f64>() * 0.2 - 0.1) * 2.0,
        );
        let direction_length_sq = rough_direction.length_squared();
        if direction_length_sq > 0.0 && rough_direction.dot(&motion) < 0.0 {
            motion = motion * -1.0;
        }
        if direction_length_sq > 0.0 {
            let size = self.entity.bounding_box.load().get_average_side_length();
            let offset = rough_direction.normalize() * (size * 0.5);
            self.entity.set_pos(position.add(&offset));
        }
        self.entity.velocity.store(motion);
    }

    fn try_merge_to_existing(world: &Arc<World>, position: Vector3<f64>, value: u32) -> bool {
        let id = rand::rng().random_range(0..MERGE_ID_SPREAD);
        let search = BoundingBox::new(
            position.sub(&Vector3::new(0.5, 0.5, 0.5)),
            position.add(&Vector3::new(0.5, 0.5, 0.5)),
        );
        for other in world.get_entities_at_box(&search) {
            let Some(orb) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            if !Self::can_merge(orb, id, value) {
                continue;
            }
            let _guard = orb
                .merge_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if orb.entity.removed.load(Ordering::SeqCst) {
                continue;
            }
            orb.count.fetch_add(1, Ordering::SeqCst);
            orb.orb_age.store(0, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn can_merge(orb: &Self, id: i32, value: u32) -> bool {
        !orb.entity.removed.load(Ordering::SeqCst)
            && (orb.entity.entity_id - id) % MERGE_ID_SPREAD == 0
            && orb.value.load(Ordering::Relaxed) == value
    }

    fn scan_for_merges(&self) {
        let world = self.entity.world.load_full();
        let search = self.entity.bounding_box.load().expand(0.5, 0.5, 0.5);
        for other in world.get_entities_at_box(&search) {
            let Some(orb) = other.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            if orb.entity.entity_id == self.entity.entity_id
                || !Self::can_merge(
                    orb,
                    self.entity.entity_id,
                    self.value.load(Ordering::Relaxed),
                )
            {
                continue;
            }
            self.merge(orb);
            if self.entity.removed.load(Ordering::SeqCst) {
                break;
            }
        }
    }

    fn merge(&self, other: &Self) {
        let (first, second) = if self.entity.entity_id < other.entity.entity_id {
            (self, other)
        } else {
            (other, self)
        };
        let _first = first
            .merge_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _second = second
            .merge_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.entity.removed.load(Ordering::SeqCst) || other.entity.removed.load(Ordering::SeqCst)
        {
            return;
        }
        let other_count = other.count.swap(0, Ordering::SeqCst);
        self.count.fetch_add(other_count, Ordering::SeqCst);
        self.orb_age
            .fetch_min(other.orb_age.load(Ordering::Relaxed), Ordering::Relaxed);
        other.entity.remove();
    }

    fn follow_nearby_player(&self) {
        let pos = self.entity.pos.load();
        let mut following = self
            .following_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keep = following.as_ref().is_some_and(|player| {
            !player.is_spectator()
                && player.get_entity().pos.load().squared_distance_to_vec(&pos)
                    <= FOLLOW_DISTANCE * FOLLOW_DISTANCE
        });
        if !keep {
            *following = self
                .entity
                .world
                .load()
                .get_closest_player(pos, FOLLOW_DISTANCE)
                .filter(|player| {
                    !player.is_spectator() && player.living_entity.health.load() > 0.0
                });
        }

        if let Some(player) = following.as_ref() {
            let player_entity = player.get_entity();
            let player_pos = player_entity.pos.load();
            let delta = Vector3::new(
                player_pos.x - pos.x,
                player_pos.y + player_entity.get_eye_height() / 2.0 - pos.y,
                player_pos.z - pos.z,
            );
            let power = 1.0 - delta.length_squared().sqrt() / FOLLOW_DISTANCE;
            let pull = delta.normalize() * (power * power * 0.1);
            self.entity
                .velocity
                .store(self.entity.velocity.load().add(&pull));
        }
    }

    const fn get_experience_value(max_value: u32) -> u32 {
        if max_value >= 2477 {
            2477
        } else if max_value >= 1237 {
            1237
        } else if max_value >= 617 {
            617
        } else if max_value >= 307 {
            307
        } else if max_value >= 149 {
            149
        } else if max_value >= 73 {
            73
        } else if max_value >= 37 {
            37
        } else if max_value >= 17 {
            17
        } else if max_value >= 7 {
            7
        } else if max_value >= 3 {
            3
        } else {
            1
        }
    }
}

impl EntityBase for ExperienceOrbEntity {
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = &self.entity;
        entity.tick(caller, server);
        if entity.removed.load(Ordering::SeqCst) {
            return;
        }
        let world = entity.world.load_full();

        let colliding = !world.is_space_empty(entity.bounding_box.load());
        let mut velo = entity.velocity.load();
        if entity.is_submerged_in_water() {
            velo = Vector3::new(velo.x * 0.99, (velo.y + 5.0e-4).min(0.06), velo.z * 0.99);
        } else if !colliding {
            velo.y -= self.get_gravity();
        }
        let block_fluid = world.get_fluid(&entity.block_pos.load());
        if block_fluid.id == Fluid::LAVA.id || block_fluid.id == Fluid::FLOWING_LAVA.id {
            let mut rng = rand::rng();
            velo = Vector3::new(
                f64::from((rng.random::<f32>() - rng.random::<f32>()) * 0.2),
                0.2,
                f64::from((rng.random::<f32>() - rng.random::<f32>()) * 0.2),
            );
        }
        entity.velocity.store(velo);

        if entity.age.load(Ordering::Relaxed) % 20 == 1 {
            self.scan_for_merges();
            if entity.removed.load(Ordering::SeqCst) {
                return;
            }
        }

        self.follow_nearby_player();
        let following = self
            .following_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        if !following && colliding {
            let bounding_box = entity.bounding_box.load();
            let moved = bounding_box.shift(entity.velocity.load());
            if !world.is_space_empty(moved) {
                let pos = entity.pos.load();
                entity.push_out_of_blocks(Vector3::new(
                    pos.x,
                    f64::midpoint(bounding_box.min.y, bounding_box.max.y),
                    pos.z,
                ));
                entity.velocity_dirty.store(true, Ordering::SeqCst);
            }
        }

        let fall_speed = entity.velocity.load().y;
        entity.move_entity(caller, entity.velocity.load());
        entity.tick_block_collisions(caller);

        let air_drag = 0.98;
        let mut friction = air_drag;
        let on_ground = entity.on_ground.load(Ordering::SeqCst);
        if on_ground {
            friction *= f64::from(entity.get_block_with_y_offset(0.999_999).1.slipperiness);
        }
        let mut velo = entity.velocity.load() * friction;
        if on_ground && fall_speed < -self.get_gravity() {
            velo.y = -fall_speed * 0.4;
        }
        entity.velocity.store(velo);

        if self.orb_age.fetch_add(1, Ordering::Relaxed) + 1 >= LIFETIME {
            entity.remove();
        }
    }

    fn init_data_tracker(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::experience_orb::DATA_VALUE,
            self.value.load(Ordering::Relaxed) as i32,
        );
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn on_player_collision(&self, player: &Arc<Player>) {
        if player.living_entity.health.load() <= 0.0 {
            return;
        }
        let can_pickup = if let Ok(mut delay) = player.experience_pick_up_delay.try_lock()
            && *delay == 0
        {
            *delay = 2;
            true
        } else {
            false
        };
        if !can_pickup {
            return;
        }
        let _guard = self
            .merge_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.entity.removed.load(Ordering::SeqCst) {
            return;
        }
        player.living_entity.pickup(&self.entity, 1);
        let remaining = player.apply_mending_from_xp(self.value.load(Ordering::Relaxed) as i32);
        if remaining > 0 {
            player.add_experience_points(remaining);
        }
        if self.count.fetch_sub(1, Ordering::SeqCst) <= 1 {
            self.entity.remove();
        }
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.entity.is_invulnerable_to(&damage_type, cause) {
            return false;
        }
        self.entity.velocity_dirty.store(true, Ordering::SeqCst);
        let health = self.health.load(Ordering::Relaxed) - amount as i32;
        self.health.store(health, Ordering::Relaxed);
        if health <= 0 {
            self.entity.remove();
        }
        true
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_short("Health", self.health.load(Ordering::Relaxed) as i16);
        nbt.put_short("Age", self.orb_age.load(Ordering::Relaxed) as i16);
        nbt.put_short("Value", self.value.load(Ordering::Relaxed) as i16);
        nbt.put_int("Count", self.count.load(Ordering::Relaxed) as i32);
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.health.store(
            i32::from(nbt.get_short("Health").unwrap_or(5)),
            Ordering::Relaxed,
        );
        self.orb_age.store(
            i32::from(nbt.get_short("Age").unwrap_or(0)),
            Ordering::Relaxed,
        );
        self.value.store(
            nbt.get_short("Value").unwrap_or(0) as u32,
            Ordering::Relaxed,
        );
        let count = nbt.get_int("Count").filter(|count| *count > 0).unwrap_or(1);
        self.count.store(count as u32, Ordering::Relaxed);
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn get_gravity(&self) -> f64 {
        0.03
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
