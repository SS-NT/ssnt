use bevy::{prelude::*, utils::HashMap};
use networking::{
    is_server,
    messaging::{MessageReceivers, MessageSender},
    scene::NetworkSceneBundle,
    spawning::ClientControls,
    visibility::{NetworkObserver, NetworkObserverBundle},
    Players,
};

use crate::movement::ForcePositionMessage;

use super::{
    health::{BrainState, BrainStateEvent},
    Body,
};

pub struct GhostPlugin;

impl Plugin for GhostPlugin {
    fn build(&self, app: &mut App) {
        if is_server(app) {
            app.init_resource::<Ghosts>().add_systems(
                Update,
                (create_ghost, return_to_body).run_if(on_event::<BrainStateEvent>()),
            );
        }
    }
}

#[derive(Resource, Default)]
struct Ghosts {
    brain_to_ghost: HashMap<Entity, Entity>,
}

#[allow(clippy::too_many_arguments)]
fn create_ghost(
    mut brain_events: EventReader<BrainStateEvent>,
    mut ghosts: ResMut<Ghosts>,
    mut controls: ResMut<ClientControls>,
    parents: Query<&Parent>,
    bodies: Query<(), With<Body>>,
    asset_server: Res<AssetServer>,
    players: Res<Players>,
    global_transforms: Query<&GlobalTransform>,
    mut commands: Commands,
    mut sender: MessageSender,
) {
    for event in brain_events.iter() {
        if event.new_state != BrainState::Dead {
            continue;
        }

        let Some(body_entity) = parents
            .iter_ancestors(event.brain)
            .find(|e| bodies.contains(*e))
        else {
            continue;
        };

        // Only spawn ghost for entities controlled by players
        let Some(player) = controls.controlling_player(body_entity) else {
            continue;
        };

        let Ok(position) = global_transforms.get(body_entity).map(|t| t.translation()) else {
            continue;
        };

        // Spawn ghost if it doesnt exist
        if !ghosts.brain_to_ghost.contains_key(&event.brain) {
            let ghost = commands
                .spawn((
                    NetworkSceneBundle {
                        scene: asset_server.load("creatures/ghost.scn.ron").into(),
                        transform: Transform::from_translation(position),
                        ..Default::default()
                    },
                    NetworkObserverBundle {
                        observer: NetworkObserver {
                            range: 1,
                            player_id: player,
                        },
                        cells: Default::default(),
                    },
                    networking::transform::ClientMovement,
                ))
                .id();
            ghosts.brain_to_ghost.insert(event.brain, ghost);
        }

        // Set player to control ghost
        controls.give_control(
            player,
            ghosts.brain_to_ghost.get(&event.brain).copied().unwrap(),
        );

        // Set new position
        // Holy shit server-movement when
        if let Some(connection) = players.get_connection(&player) {
            sender.send_with_priority(
                &ForcePositionMessage {
                    position,
                    rotation: Quat::IDENTITY,
                },
                MessageReceivers::Single(connection),
                10,
            );
        }
    }
}

fn return_to_body(
    mut brain_events: EventReader<BrainStateEvent>,
    mut ghosts: ResMut<Ghosts>,
    mut controls: ResMut<ClientControls>,
    parents: Query<&Parent>,
    bodies: Query<(), With<Body>>,
    mut commands: Commands,
) {
    for event in brain_events.iter() {
        if event.new_state == BrainState::Dead {
            continue;
        }

        let Some(body_entity) = parents
            .iter_ancestors(event.brain)
            .find(|e| bodies.contains(*e))
        else {
            continue;
        };

        let Some(ghost_entity) = ghosts.brain_to_ghost.remove(&event.brain) else {
            continue;
        };

        let Some(player) = controls.controlling_player(ghost_entity) else {
            continue;
        };

        controls.give_control(player, body_entity);
        commands.entity(ghost_entity).despawn_recursive();
    }
}
