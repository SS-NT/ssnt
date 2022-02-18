use std::collections::{hash_map::Entry, VecDeque};

use bevy::{
    core::Time,
    math::{Quat, Vec3},
    prelude::{
        warn, App, Component, Local, ParallelSystemDescriptorCoercion, Plugin, Query, Res, ResMut,
        SystemLabel, SystemSet, Transform, Without,
    },
    utils::HashMap,
};
use bevy_networking_turbulence::NetworkResource;
use bevy_rapier3d::{physics::PhysicsSystems, prelude::{RigidBodyPositionComponent, RigidBodyVelocityComponent}};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    spawning::{ClientControlled, SpawningSystems},
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct Acknowledgment {
    identity: NetworkIdentity,
    sequence_number: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TransformUpdate {
    identity: NetworkIdentity,
    sequence_number: u16,
    // TODO: Add delta compression
    position: Option<Vec3>,
    rotation: Option<Quat>,
    linear_velocity: Option<Vec3>,
    angular_velocity: Option<Vec3>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) enum TransformMessage {
    Ack(Acknowledgment),
    Update(TransformUpdate),
}

/// Stores per-client data regarding [`NetworkTransform`] synchronisation
#[derive(Default)]
struct ClientData {
    /// The last time a network update was sent
    last_sent: f32,
    /// The last time an ack was received
    last_ack: f32,
    /// The sequence number of the last ack
    last_sequence: u16,
}

/// Sends transform changes to clients
#[derive(Component)]
pub struct NetworkTransform {
    /// How many times this transform is sent per second
    pub update_rate: f32,
    /// How much the position needs to move to be considered changed
    pub position_threshold: f32,
    /// How much the rotation needs to change to be considered changed
    pub rotation_threshold: f32,
    /// How long to wait for a position ack before retransmitting.
    /// This is a multiplicator, 1 = 1 x RTT.
    /// Retransmission is only necessary when the update rate is below RTT or the transform has stopped moving.
    pub retransmission_multiplicator: f32,
    sent_updates: VecDeque<TransformUpdate>,
    sent_queue_length: usize,
    client_data: HashMap<ConnectionId, ClientData>,
    last_sequence: u16,
    last_update: f32,
    last_change: f32,
}

impl Default for NetworkTransform {
    fn default() -> Self {
        Self {
            update_rate: 30.0,
            position_threshold: 0.01,
            rotation_threshold: 0.01,
            retransmission_multiplicator: 2.0,
            sent_updates: VecDeque::with_capacity(30),
            sent_queue_length: 30,
            client_data: Default::default(),
            last_sequence: Default::default(),
            last_update: Default::default(),
            last_change: Default::default(),
        }
    }
}

impl NetworkTransform {
    fn add_update(&mut self, update: TransformUpdate) {
        if self.sent_updates.len() >= self.sent_queue_length {
            self.sent_updates.pop_front();
        }

        self.sent_updates.push_back(update);
    }

    fn get_sequence_number(&mut self) -> u16 {
        let seq = self.last_sequence + 1;
        self.last_sequence = seq;
        seq
    }

    // How many updates have been skipped due to the transform not changing
    fn updates_skipped(&self) -> f32 {
        let update_diff = self.last_update - self.last_change;
        update_diff / (1.0 / self.update_rate)
    }
}

fn update_transform(
    mut query: Query<(&mut NetworkTransform, &Transform, &NetworkIdentity, Option<&RigidBodyVelocityComponent>)>,
    time: Res<Time>,
    visibilities: Res<NetworkVisibilities>,
    mut network: ResMut<NetworkResource>,
) {
    let seconds = time.time_since_startup().as_secs_f32();
    for (mut networked, transform, identity, velocity) in query.iter_mut() {
        let networked: &mut NetworkTransform = &mut networked;
        // Respect update rate
        if networked.last_update + 1.0 / networked.update_rate > seconds {
            continue;
        }

        networked.last_update = seconds;

        // Compare current values
        let last_update = networked.sent_updates.back();
        let last_position = last_update.and_then(|u| u.position).unwrap_or(Vec3::ZERO);
        let last_rotation = last_update
            .and_then(|u| u.rotation)
            .unwrap_or(Quat::IDENTITY);

        let new_position: Vec3 = transform.translation;
        let update_position =
            !new_position.abs_diff_eq(last_position, networked.position_threshold);

        let new_rotation: Quat = transform.rotation;
        let update_rotation =
            !new_rotation.abs_diff_eq(last_rotation, networked.rotation_threshold);

        // Exit early if nothing to send
        if !update_position && !update_rotation {
            continue;
        }

        // Construct the update message
        let update = TransformUpdate {
            identity: *identity,
            sequence_number: networked.get_sequence_number(),
            position: if update_position {
                Some(new_position)
            } else {
                None
            },
            rotation: if update_rotation {
                Some(new_rotation)
            } else {
                None
            },
            linear_velocity: velocity.map(|v| v.linvel.into()),
            angular_velocity: velocity.map(|v| v.angvel.into()),
        };
        networked.add_update(update.clone());

        // Send to all observers
        if let Some(visibility) = visibilities.visibility.get(identity) {
            for connection in visibility.observers() {
                network
                    .send_message(connection.0, TransformMessage::Update(update.clone()))
                    .unwrap();
            }
        }

        networked.last_change = seconds;
    }
}

/// Retransmits last update for dropped packets and new observers.
/// This only happens when the transform is not moving, otherwise old updates can be dropped.
fn handle_retransmission(
    mut query: Query<(&mut NetworkTransform, &NetworkIdentity)>,
    time: Res<Time>,
    visibilities: Res<NetworkVisibilities>,
    mut network: ResMut<NetworkResource>,
) {
    let seconds = time.time_since_startup().as_secs_f32();
    for (mut networked, identity) in query.iter_mut() {
        let networked: &mut NetworkTransform = &mut networked;

        // We only repeat the last update if the transform has remained the same for two updates
        if networked.updates_skipped() <= 2.0 {
            continue;
        }

        // TODO: Use actual RTT
        let time_offset = networked.retransmission_multiplicator * 0.1;

        let visibility = match visibilities.visibility.get(identity) {
            Some(v) => v,
            None => continue,
        };
        let last_update = match networked.sent_updates.back() {
            Some(u) => u,
            None => continue,
        };

        // Retransmit for missed acks
        for (connection, data) in networked.client_data.iter_mut() {
            if data.last_sequence == networked.last_sequence {
                continue;
            }

            if !visibility.has_observer(connection) {
                continue;
            }

            if data.last_ack + time_offset > seconds || data.last_sent + time_offset > seconds {
                continue;
            }

            network
                .send_message(connection.0, TransformMessage::Update(last_update.clone()))
                .unwrap();
            data.last_sent = seconds;
        }

        // Transmit for new observers
        for connection in visibility.observers().iter() {
            let entry = networked.client_data.entry(*connection);
            if let Entry::Vacant(entry) = entry {
                entry.insert(ClientData {
                    last_sent: seconds,
                    ..Default::default()
                });
                network
                    .send_message(connection.0, TransformMessage::Update(last_update.clone()))
                    .unwrap();
            }
        }
    }
}

const TRANSFORM_STILL_RESYNC_WAIT: f32 = 5.0;

/// Syncs the transform from time to time if it's not changing.
/// This fixes physic desyncs when movement is stopped.
fn handle_occasional_sync(
    mut query: Query<(&mut NetworkTransform, &NetworkIdentity)>,
    time: Res<Time>,
    visibilities: Res<NetworkVisibilities>,
    mut network: ResMut<NetworkResource>,
) {
    let seconds = time.time_since_startup().as_secs_f32();

    for (mut networked, identity) in query.iter_mut() {
        if networked.last_change + TRANSFORM_STILL_RESYNC_WAIT > seconds {
            continue;
        }

        let visibility = match visibilities.visibility.get(identity) {
            Some(v) => v,
            None => continue,
        };
        let last_update = match networked.sent_updates.back() {
            Some(u) => u.clone(),
            None => continue,
        };

        for (connection, data) in networked.client_data.iter_mut() {
            if !visibility.has_observer(connection) {
                continue;
            }

            network
                .send_message(connection.0, TransformMessage::Update(last_update.clone()))
                .unwrap();
            data.last_sent = seconds;
        }

        networked.last_change = seconds;
    }
}

/// Process acknowledgments from clients
fn handle_acks(
    mut query: Query<&mut NetworkTransform>,
    mut network: ResMut<NetworkResource>,
    identities: Res<NetworkIdentities>,
    time: Res<Time>,
) {
    let seconds = time.time_since_startup().as_secs_f32();
    for (handle, connection) in network.connections.iter_mut() {
        let channels = connection.channels().unwrap();
        while let Some(message) = channels.recv::<TransformMessage>() {
            match message {
                TransformMessage::Ack(ack) => {
                    let entity = match identities.get_entity(ack.identity) {
                        Some(e) => e,
                        None => {
                            warn!(
                                "Received transform ack for non-existent {:?} from {}",
                                ack.identity, handle
                            );
                            continue;
                        }
                    };

                    let mut transform = match query.get_mut(entity) {
                        Ok(t) => t,
                        Err(_) => {
                            warn!("Received transform ack for entity without network transform {:?} from {}", entity, handle);
                            continue;
                        }
                    };

                    let mut data = transform
                        .client_data
                        .entry(ConnectionId(*handle))
                        .or_default();
                    if data.last_sequence < ack.sequence_number {
                        data.last_sequence = ack.sequence_number;
                        data.last_ack = seconds;
                    }
                }
                _ => {
                    warn!("Received invalid transform message from {}", handle);
                }
            }
        }
    }
}

/// Receives transform updates from the network
#[derive(Component, Default)]
pub struct NetworkedTransform {
    last_update: Option<TransformUpdate>,
}

const UPDATE_BUFFER_SIZE: usize = 150;
/// Stores transform updates that could not be applied
struct BufferedTransformUpdates {
    updates: VecDeque<TransformUpdate>,
}

impl Default for BufferedTransformUpdates {
    fn default() -> Self {
        Self {
            updates: VecDeque::with_capacity(UPDATE_BUFFER_SIZE),
        }
    }
}

impl BufferedTransformUpdates {
    fn add(&mut self, update: TransformUpdate) {
        if self.updates.len() >= UPDATE_BUFFER_SIZE {
            self.updates.pop_front();
            warn!(
                "Dropped transform update (buffer full) for {:?}",
                update.identity
            );
        }

        self.updates.push_back(update);
    }
}

/// Receives transform messages and sends acknowledgments
fn handle_transform_messages(
    mut network: ResMut<NetworkResource>,
    mut buffer: ResMut<BufferedTransformUpdates>,
    mut acknowledgments: Local<Vec<Acknowledgment>>,
) {
    if network.connections.is_empty() {
        return;
    }

    for (_, connection) in network.connections.iter_mut() {
        let channels = connection.channels().unwrap();
        while let Some(message) = channels.recv::<TransformMessage>() {
            match message {
                TransformMessage::Update(update) => {
                    acknowledgments.push(Acknowledgment {
                        identity: update.identity,
                        sequence_number: update.sequence_number,
                    });
                    buffer.add(update);
                }
                _ => panic!("Unsupported transform message"),
            }
        }
    }

    let connection = *network.connections.iter().next().unwrap().0;
    for ack in acknowledgments.drain(..) {
        network
            .send_message(connection, TransformMessage::Ack(ack))
            .unwrap();
    }
}

/// Apply the buffered transform messages to the relevant entities
fn apply_buffered_updates(
    mut buffer: ResMut<BufferedTransformUpdates>,
    mut query: Query<(&mut NetworkedTransform, Option<&ClientControlled>)>,
    identities: Res<NetworkIdentities>,
    mut unique_updates: Local<HashMap<NetworkIdentity, TransformUpdate>>,
) {
    buffer.updates.retain(|update| {
        let entity = match identities.get_entity(update.identity) {
            Some(e) => e,
            None => return true,
        };

        let (mut networked, client_controlled) = match query.get_mut(entity) {
            Ok(n) => n,
            Err(_) => {
                return true;
            }
        };

        // TODO: Remove `if` once movement is server authoritative
        if client_controlled.is_none() {
            networked.last_update = Some(update.clone());
        }

        false
    });

    // Deduplicate the non-applied updates
    for update in buffer.updates.drain(..) {
        match unique_updates.entry(update.identity) {
            Entry::Occupied(mut o) => {
                let existing = o.get();
                // Replace if same identity and newer sequence number
                if existing.sequence_number < update.sequence_number {
                    o.insert(update);
                }
            },
            Entry::Vacant(v) => {
                v.insert(update);
            },
        }
    }
    buffer.updates.extend(unique_updates.drain().map(|(_, u)| u));
}

/// Applies transform updates to entities without physics simulation
fn sync_networked_transform(
    mut query: Query<
        (&mut NetworkedTransform, &mut Transform),
        Without<RigidBodyPositionComponent>,
    >,
) {
    for (mut networked, mut transform) in query.iter_mut() {
        if let Some(update) = &networked.last_update {
            if let Some(position) = update.position {
                transform.translation = position;
            }

            if let Some(rotation) = update.rotation {
                transform.rotation = rotation;
            }
        }

        networked.last_update = None;
    }
}

/// Applies transform updates to entities with physics
fn sync_networked_transform_physics(
    mut query: Query<(&mut NetworkedTransform, &mut RigidBodyPositionComponent, &mut RigidBodyVelocityComponent)>,
) {
    for (mut transform, mut rigidbody, mut velocity) in query.iter_mut() {
        if let Some(update) = &transform.last_update {
            if let Some(position) = update.position {
                rigidbody.position.translation = position.into();
            }

            if let Some(rotation) = update.rotation {
                rigidbody.position.rotation = rotation.into();
            }

            if let Some(linear_velocity) = update.linear_velocity {
                velocity.linvel = linear_velocity.into();
            }

            if let Some(angular_velocity) = update.angular_velocity {
                velocity.angvel = angular_velocity.into();
            }
        }

        transform.last_update = None;
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
enum ClientTransformSystem {
    /// Receive transform update messages
    ReceiveMessages,
    /// Apply buffered transform updates
    ApplyBuffer,
    /// Sync updates with transforms and physics
    Sync,
}

pub(crate) struct TransformPlugin;

impl Plugin for TransformPlugin {
    fn build(&self, app: &mut App) {
        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.add_system_set(
                SystemSet::new()
                    .after(SpawningSystems::Spawn)
                    .with_system(update_transform)
                    .with_system(handle_retransmission)
                    .with_system(handle_acks)
                    .with_system(handle_occasional_sync),
            );
        } else {
            app.init_resource::<BufferedTransformUpdates>()
                .add_system(handle_transform_messages.label(ClientTransformSystem::ReceiveMessages))
                .add_system(
                    apply_buffered_updates
                        .label(ClientTransformSystem::ApplyBuffer)
                        .after(ClientTransformSystem::ReceiveMessages)
                        .after(SpawningSystems::Spawn),
                )
                .add_system(
                    sync_networked_transform
                        .label(ClientTransformSystem::Sync)
                        .after(ClientTransformSystem::ApplyBuffer),
                )
                .add_system(
                    sync_networked_transform_physics
                        .label(ClientTransformSystem::Sync)
                        .after(ClientTransformSystem::ApplyBuffer)
                        .before(PhysicsSystems::StepWorld),
                );
        }
    }
}
