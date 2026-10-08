use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{
    entity::{Entity, EntityBase},
    item::potion::{PotionContents, apply_instantaneous_effect, is_instantaneous},
    server::Server,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

type EffectEntry = (&'static StatusEffect, i32, u8, bool, bool, bool);

const TIME_BETWEEN_APPLICATIONS: i32 = 5;
const MAX_RADIUS: f32 = 32.0;
const MINIMAL_RADIUS: f32 = 0.5;
const HEIGHT: f64 = 0.5;
pub const INFINITE_DURATION: i32 = -1;
/// The scale vanilla passes to `applyInstantaneousEffect` for cloud victims.
const INSTANT_EFFECT_SCALE: f64 = 0.5;

#[derive(Clone)]
struct ParticleMeta {
    particle_id: pumpkin_protocol::codec::var_int::VarInt,
    data: Box<[u8]>,
}

impl pumpkin_protocol::java::client::play::MetadataSerializer for ParticleMeta {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &pumpkin_util::version::JavaMinecraftVersion,
    ) -> Result<(), pumpkin_protocol::ser::WritingError> {
        use pumpkin_protocol::ser::NetworkWriteExt;
        writer.write_var_int(&self.particle_id)?;
        writer.write_slice(&self.data)
    }
}

/// The particle a cloud shows instead of its potion colour.
#[derive(Clone, Copy)]
pub enum CloudParticle {
    /// `PowerParticleOption` for `minecraft:dragon_breath`.
    DragonBreath { power: f32 },
}

/// Mutable settings of a cloud, named after vanilla `AreaEffectCloud`'s fields.
pub struct CloudSettings {
    pub potion: ItemStack,
    pub effects: Vec<EffectEntry>,
    pub custom_particle: Option<CloudParticle>,
    pub potion_duration_scale: f32,
    pub duration: i32,
    pub wait_time: i32,
    pub reapplication_delay: i32,
    pub duration_on_use: i32,
    pub radius_on_use: f32,
    pub radius_per_tick: f32,
    pub radius: f32,
    pub owner_id: Option<i32>,
    age: i32,
    /// Entity id to the tick its reapplication delay ends.
    victims: HashMap<i32, i32>,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            potion: ItemStack::EMPTY.clone(),
            effects: Vec::new(),
            custom_particle: None,
            potion_duration_scale: 1.0,
            duration: INFINITE_DURATION,
            wait_time: 20,
            reapplication_delay: 20,
            duration_on_use: 0,
            radius_on_use: 0.0,
            radius_per_tick: 0.0,
            radius: 3.0,
            owner_id: None,
            age: 0,
            victims: HashMap::new(),
        }
    }
}

/// Vanilla `AreaEffectCloud`.
pub struct AreaEffectCloudEntity {
    pub entity: Entity,
    settings: Mutex<CloudSettings>,
}

impl AreaEffectCloudEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        entity
            .no_physics
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Self {
            entity,
            settings: Mutex::new(CloudSettings::default()),
        }
    }

    /// Settings for a cloud that has not been spawned yet.
    pub fn settings_mut(&mut self) -> &mut CloudSettings {
        self.settings
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn particle_meta(settings: &CloudSettings) -> ParticleMeta {
        if let Some(CloudParticle::DragonBreath { power }) = settings.custom_particle {
            return ParticleMeta {
                particle_id: pumpkin_protocol::codec::var_int::VarInt(
                    Particle::DragonBreath as i32,
                ),
                data: Box::new(power.to_be_bytes()),
            };
        }
        let color = PotionContents::color_of(&settings.potion, &settings.effects) | (0xFF << 24);
        ParticleMeta {
            particle_id: pumpkin_protocol::codec::var_int::VarInt(Particle::EntityEffect as i32),
            data: Box::new(color.to_be_bytes()),
        }
    }

    fn set_radius(&self, settings: &mut CloudSettings, radius: f32) {
        settings.radius = radius.clamp(0.0, MAX_RADIUS);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
            settings.radius,
        );
    }

    fn victim_candidates(&self, radius: f32) -> Vec<Arc<dyn EntityBase>> {
        let pos = self.entity.pos.load();
        let r = f64::from(radius);
        let aabb = BoundingBox::new(
            Vector3::new(pos.x - r, pos.y, pos.z - r),
            Vector3::new(pos.x + r, pos.y + HEIGHT, pos.z + r),
        );
        let world = self.entity.world.load();
        let mut candidates = world.get_entities_at_box(&aabb);
        candidates.extend(
            world
                .get_players_at_box(&aabb)
                .into_iter()
                .filter(|player| !player.is_spectator())
                .map(|player| player as Arc<dyn EntityBase>),
        );
        candidates
    }
}

impl EntityBase for AreaEffectCloudEntity {
    fn init_data_tracker(&self) {
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::PARTICLE,
            Self::particle_meta(&settings),
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::RADIUS,
            settings.radius,
        );
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::area_effect_cloud::WAITING,
            settings.age < settings.wait_time,
        );
    }

    #[expect(clippy::too_many_lines, reason = "mirrors vanilla serverTick")]
    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.entity.tick(caller, server);
        let mut settings = self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        settings.age += 1;
        let age = settings.age;
        if settings.duration != INFINITE_DURATION && age - settings.wait_time >= settings.duration {
            drop(settings);
            self.entity.remove();
            return;
        }

        let should_wait = age < settings.wait_time;
        if age == settings.wait_time {
            self.entity.set_synced_data(
                pumpkin_data::tracked_data::area_effect_cloud::WAITING,
                should_wait,
            );
        }
        if should_wait {
            return;
        }

        let mut radius = settings.radius;
        if settings.radius_per_tick != 0.0 {
            radius += settings.radius_per_tick;
            if radius < MINIMAL_RADIUS {
                drop(settings);
                self.entity.remove();
                return;
            }
            self.set_radius(&mut settings, radius);
        }

        if age % TIME_BETWEEN_APPLICATIONS != 0 {
            return;
        }
        settings.victims.retain(|_, until| age < *until);
        if settings.effects.is_empty() {
            settings.victims.clear();
            return;
        }

        let scale = settings.potion_duration_scale;
        let effects: Vec<EffectEntry> = settings
            .effects
            .iter()
            .map(|&(effect, duration, amplifier, ambient, particles, icon)| {
                let scaled = if duration == INFINITE_DURATION {
                    duration
                } else {
                    ((duration as f32 * scale).floor() as i32).max(1)
                };
                (effect, scaled, amplifier, ambient, particles, icon)
            })
            .collect();
        let world = self.entity.world.load_full();
        let owner = settings.owner_id.and_then(|id| world.get_entity_by_id(id));
        let pos = self.entity.pos.load();

        for candidate in self.victim_candidates(radius) {
            let Some(living) = candidate.get_living_entity() else {
                continue;
            };
            let id = living.entity.entity_id;
            if settings.victims.contains_key(&id)
                || !living.is_affected_by_potions()
                || !effects
                    .iter()
                    .any(|(effect, ..)| living.can_be_affected(effect))
            {
                continue;
            }
            let target_pos = living.entity.pos.load();
            let (dx, dz) = (target_pos.x - pos.x, target_pos.z - pos.z);
            if dx * dx + dz * dz > f64::from(radius * radius) {
                continue;
            }

            let delay = settings.reapplication_delay;
            settings.victims.insert(id, age + delay);
            for &(effect, duration, amplifier, ambient, show_particles, show_icon) in &effects {
                if is_instantaneous(effect) {
                    apply_instantaneous_effect(
                        candidate.as_ref(),
                        effect,
                        amplifier,
                        INSTANT_EFFECT_SCALE,
                        Some(self),
                        owner.as_deref(),
                    );
                } else {
                    living.add_effect(pumpkin_data::potion::Effect {
                        effect_type: effect,
                        duration,
                        amplifier,
                        ambient,
                        show_particles,
                        show_icon,
                        blend: false,
                    });
                }
            }

            if settings.radius_on_use != 0.0 {
                radius += settings.radius_on_use;
                if radius < MINIMAL_RADIUS {
                    drop(settings);
                    self.entity.remove();
                    return;
                }
                self.set_radius(&mut settings, radius);
            }
            if settings.duration_on_use != 0 && settings.duration != INFINITE_DURATION {
                settings.duration += settings.duration_on_use;
                if settings.duration <= 0 {
                    drop(settings);
                    self.entity.remove();
                    return;
                }
            }
        }
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        nbt.put_int("Age", settings.age);
        nbt.put_int("Duration", settings.duration);
        nbt.put_int("WaitTime", settings.wait_time);
        nbt.put_int("ReapplicationDelay", settings.reapplication_delay);
        nbt.put_int("DurationOnUse", settings.duration_on_use);
        nbt.put_float("RadiusOnUse", settings.radius_on_use);
        nbt.put_float("RadiusPerTick", settings.radius_per_tick);
        nbt.put_float("Radius", settings.radius);
        if settings.potion_duration_scale != 1.0 {
            nbt.put_float("potion_duration_scale", settings.potion_duration_scale);
        }
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let mut settings = self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        settings.age = nbt.get_int("Age").unwrap_or(0);
        settings.duration = nbt.get_int("Duration").unwrap_or(INFINITE_DURATION);
        settings.wait_time = nbt.get_int("WaitTime").unwrap_or(20);
        settings.reapplication_delay = nbt.get_int("ReapplicationDelay").unwrap_or(20);
        settings.duration_on_use = nbt.get_int("DurationOnUse").unwrap_or(0);
        settings.radius_on_use = nbt.get_float("RadiusOnUse").unwrap_or(0.0);
        settings.radius_per_tick = nbt.get_float("RadiusPerTick").unwrap_or(0.0);
        settings.radius = nbt
            .get_float("Radius")
            .unwrap_or(3.0)
            .clamp(0.0, MAX_RADIUS);
        settings.potion_duration_scale = nbt.get_float("potion_duration_scale").unwrap_or(1.0);
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
