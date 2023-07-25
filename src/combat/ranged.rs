use std::time::Duration;

use bevy::{prelude::*, reflect::TypeUuid};
use bevy_rapier3d::prelude::{CollisionGroups, QueryFilter, RapierContext};
use networking::{
    component::AppExt,
    is_server,
    messaging::{AppExt as MessageAppExt, MessageEvent, MessageReceivers, MessageSender},
    variable::{NetworkVar, ServerVar},
    Networked,
};
use serde::{Deserialize, Serialize};

use crate::{body::Limb, combat::damage::*, GameState};

use super::CombatInputEvent;

pub struct RangedPlugin;

impl Plugin for RangedPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Gun>()
            .add_networked_component::<Gun, GunClient>()
            .add_network_message::<GunShotMessage>();

        if is_server(app) {
            app.add_systems(Update, shoot_gun);
        } else {
            app.add_systems(
                Update,
                client_handle_gun_shot_effects.run_if(in_state(GameState::Game)),
            );
        }
    }
}

/// A ranged weapon that shoots projectiles
#[derive(Component, Reflect, Networked)]
#[reflect(Component)]
#[networked(client = "GunClient")]
struct Gun {
    time_between_shots: Duration,

    #[reflect(ignore)]
    next_shot_time: NetworkVar<f32>,
}

impl Default for Gun {
    fn default() -> Self {
        Self {
            time_between_shots: Duration::from_secs_f32(0.1),
            next_shot_time: NetworkVar::from_default(0.0),
        }
    }
}

fn shoot_gun(
    mut input: EventReader<CombatInputEvent>,
    mut guns: Query<&mut Gun>,
    time: Res<Time>,
    rapier: Res<RapierContext>,
    limbs: Query<&Limb>,
    players: Query<&crate::body::Body>,
    mut commands: Commands,
    mut sender: MessageSender,
) {
    for event in input.iter() {
        if !event.input.primary_attack {
            continue;
        }

        let Some(wielded_weapon) = event.wielded_weapon else {
            continue;
        };

        let Ok(mut gun) = guns.get_mut(wielded_weapon) else {
            continue;
        };

        let elapsed = time.elapsed_seconds();
        if *gun.next_shot_time > elapsed {
            continue;
        }

        // Shoot
        let target_position = event.input.aim.target_position;
        // Hack: to shoot further up and not on ground level
        let mut origin = event.input.aim.origin + Vec3::new(0.0, 0.7, 0.0);
        let mut direction = (target_position - origin).normalize_or_zero();
        // Don't aim up or down for now
        direction.y = 0.;
        // Prevent player from hitting themselves
        origin += direction * 0.5;

        bevy::log::info!(direction = ?direction, position = ?origin, "Shooting");
        let filter = QueryFilter::new().groups(CollisionGroups::new(
            physics::RAYCASTING_GROUP,
            physics::DEFAULT_GROUP | physics::LIMB_GROUP,
        ));
        if let Some((hit_entity, toi)) = rapier.cast_ray(origin, direction, 20.0, false, filter) {
            let has_limb = limbs.contains(hit_entity);
            let has_player = players.contains(hit_entity);
            let position = origin + direction * toi;
            bevy::log::info!(has_limb, has_player, position = ?position, "Hit");

            commands.spawn((
                Attack,
                AffectedEntity(hit_entity),
                // TODO: Grab from weapon and ammo used
                KineticDamage {
                    mass: 0.115,
                    velocity: 400.0,
                    shape: KineticShape::Point,
                },
            ));
            // TODO: Attacks are not yet automatically deleted

            // TODO: Maybe handle with entity?
            // TODO: Don't send to all players, only in range
            sender.send(
                &GunShotMessage {
                    origin,
                    hit: position,
                },
                MessageReceivers::AllPlayers,
            );
        }

        *gun.next_shot_time = elapsed + gun.time_between_shots.as_secs_f32();
    }
}

#[derive(Component, Networked, TypeUuid)]
#[networked(server = "Gun")]
#[uuid = "aab5eca9-9ca6-4837-8496-2c4d066009d9"]
struct GunClient {
    next_shot_time: ServerVar<f32>,
}

impl Default for GunClient {
    fn default() -> Self {
        Self {
            next_shot_time: ServerVar::from_default(0.0),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct GunShotMessage {
    origin: Vec3,
    hit: Vec3,
}

const BULLET_TRACER_VISIBLE_SECONDS: f32 = 0.5;

fn client_handle_gun_shot_effects(
    mut messages: EventReader<MessageEvent<GunShotMessage>>,
    mut current: Local<Vec<(f32, GunShotMessage)>>,
    time: Res<Time>,
    mut gizmos: Gizmos,
) {
    let now = time.elapsed_seconds();
    for event in messages.iter() {
        let message = event.message;
        current.push((now, message));
    }

    current.retain(|(time, _)| now - time < BULLET_TRACER_VISIBLE_SECONDS);

    for (_, message) in current.iter() {
        gizmos.line(message.origin, message.hit, Color::RED);
    }
}
