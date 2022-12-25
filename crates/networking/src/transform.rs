use std::collections::VecDeque;

use bevy::{
    math::{Quat, Vec3},
    prelude::*,
    reflect::Reflect,
    transform::TransformSystem,
    utils::{hashbrown::hash_map::Entry, HashMap},
};
use bevy_rapier3d::prelude::{RigidBody, Velocity};
use bevy_renet::{
    renet::{RenetClient, RenetServer},
    run_if_client_connected,
};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{deserialize, serialize_once, Channel},
    spawning::{ClientControlled, ServerEntityEvent, SpawningSystems},
    time::{ClientNetworkTime, ServerNetworkTime, TimeSystem},
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct Acknowledgment {
    identity: NetworkIdentity,
    sequence_number: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TransformUpdate {
    identity: NetworkIdentity,
    /// The server tick this update was created
    sequence_number: u32,
    // TODO: Add delta compression
    position: Option<Vec3>,
    rotation: Option<Quat>,
    linear_velocity: Option<Vec3>,
    angular_velocity: Option<Vec3>,
}

impl TransformUpdate {
    /// Interpolates between two snapshots at the given tick
    fn interpolate(&self, other: &TransformUpdate, tick: f32) -> Self {
        assert_eq!(self.identity, other.identity);

        // Swap direction if necessary
        let mut from = self;
        let mut to = other;
        if from.sequence_number > to.sequence_number {
            std::mem::swap(&mut from, &mut to);
        }

        // Calculate at which time point we are between the updates
        let distance = to.sequence_number - from.sequence_number;
        let t = (tick - from.sequence_number as f32) / distance as f32;
        debug_assert!((0.0..=1.0).contains(&t));

        let position = Self::interpolate_component(from.position, to.position, t, Vec3::lerp);
        let rotation = Self::interpolate_component(from.rotation, to.rotation, t, Quat::lerp);
        let linear_velocity =
            Self::interpolate_component(from.linear_velocity, to.linear_velocity, t, Vec3::lerp);
        let angular_velocity =
            Self::interpolate_component(from.angular_velocity, to.angular_velocity, t, Vec3::lerp);

        TransformUpdate {
            identity: from.identity,
            sequence_number: tick as u32,
            position,
            rotation,
            linear_velocity,
            angular_velocity,
        }
    }

    /// Interpolates two values
    fn interpolate_component<T>(
        from: Option<T>,
        to: Option<T>,
        t: f32,
        lerp: impl Fn(T, T, f32) -> T,
    ) -> Option<T> {
        match from {
            Some(from) => {
                if let Some(to) = to {
                    Some(lerp(from, to, t))
                } else {
                    Some(from)
                }
            }
            None => to,
        }
    }
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
    last_sequence: u32,
}

#[derive(Default)]
struct TransformSnapshot {
    position: Option<Vec3>,
    rotation: Option<Quat>,
}

/// Sends transform changes to clients
#[derive(Component, Reflect)]
#[reflect(Component)]
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
    /// All transform information when it was last updated
    #[reflect(ignore)]
    full_snapshot: TransformSnapshot,
    #[reflect(ignore)]
    sent_updates: VecDeque<TransformUpdate>,
    sent_queue_length: usize,
    #[reflect(ignore)]
    client_data: HashMap<ConnectionId, ClientData>,
    /// The sequence number that was last created
    last_sequence: u32,
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
            full_snapshot: Default::default(),
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

        self.last_sequence = update.sequence_number;
        self.sent_updates.push_back(update);
    }

    // How many updates have been skipped due to the transform not changing
    fn updates_skipped(&self) -> f32 {
        let update_diff = self.last_update - self.last_change;
        update_diff / (1.0 / self.update_rate)
    }
}

fn update_transform(
    mut query: Query<(
        &mut NetworkTransform,
        &Transform,
        &NetworkIdentity,
        Option<&Velocity>,
    )>,
    time: Res<Time>,
    visibilities: Res<NetworkVisibilities>,
    mut server: ResMut<RenetServer>,
    network_time: Res<ServerNetworkTime>,
) {
    let seconds = time.raw_elapsed_seconds();
    for (mut networked, transform, identity, velocity) in query.iter_mut() {
        let networked: &mut NetworkTransform = &mut networked;
        // Respect update rate
        if networked.last_update + 1.0 / networked.update_rate > seconds {
            continue;
        }

        networked.last_update = seconds;

        // Compare values to the one last sent
        let last_position = networked.full_snapshot.position;
        let last_rotation = networked.full_snapshot.rotation;

        let new_position: Vec3 = transform.translation;
        let update_position = last_position.is_none()
            || !new_position.abs_diff_eq(last_position.unwrap(), networked.position_threshold);

        let new_rotation: Quat = transform.rotation;
        let update_rotation = last_rotation.is_none()
            || !new_rotation.abs_diff_eq(last_rotation.unwrap(), networked.rotation_threshold);

        // Exit early if transform did not significantly change
        if !update_position && !update_rotation {
            continue;
        }

        // Update the full snapshot if we're going to send the information
        if update_position {
            networked.full_snapshot.position = Some(new_position);
        }

        if update_rotation {
            networked.full_snapshot.rotation = Some(new_rotation);
        }

        // Construct the update message
        let update = TransformUpdate {
            identity: *identity,
            // TODO: Move this into a message at the start of a packet
            sequence_number: network_time.current_tick(),
            // TODO: Here we would need to do delta compression for each client, as they may have dropped the last packet
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
            linear_velocity: velocity.map(|v| v.linvel),
            angular_velocity: velocity.map(|v| v.angvel),
        };
        networked.add_update(update.clone());

        // Send to all observers
        if let Some(visibility) = visibilities.visibility.get(identity) {
            let message = TransformMessage::Update(update);
            let serialized = serialize_once(&message);
            for connection in visibility.observers() {
                server.send_message(connection.0, Channel::Transforms.id(), serialized.clone());
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
    mut server: ResMut<RenetServer>,
) {
    let seconds = time.raw_elapsed_seconds();
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

        let serialized = serialize_once(&TransformMessage::Update(last_update.clone()));
        // Retransmit for missed acks
        for (connection, data) in networked.client_data.iter_mut() {
            if data.last_sequence == networked.last_sequence {
                continue;
            }

            if !visibility.has_observer(connection) {
                // TODO: remove client data after some time
                continue;
            }

            if data.last_ack + time_offset > seconds || data.last_sent + time_offset > seconds {
                continue;
            }

            server.send_message(connection.0, Channel::Transforms.id(), serialized.clone());
            data.last_sent = seconds;
        }

        // Transmit for new observers
        for connection in visibility.observers() {
            let entry = networked.client_data.entry(*connection);
            if let Entry::Vacant(entry) = entry {
                entry.insert(ClientData {
                    last_sent: seconds,
                    ..Default::default()
                });
                server.send_message(connection.0, Channel::Transforms.id(), serialized.clone());
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
    mut server: ResMut<RenetServer>,
) {
    let seconds = time.raw_elapsed_seconds();

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

        let serialized = serialize_once(&TransformMessage::Update(last_update.clone()));
        for (connection, data) in networked.client_data.iter_mut() {
            if !visibility.has_observer(connection) {
                continue;
            }

            server.send_message(connection.0, Channel::Transforms.id(), serialized.clone());
            data.last_sent = seconds;
        }

        networked.last_change = seconds;
    }
}

/// Sends the newest position to clients that just had the object enter their visibility/was spawned.
/// If we didn't do this, networked objects would appear at the world origin for a few milliseconds.
fn handle_newly_spawned(
    mut events: EventReader<ServerEntityEvent>,
    query: Query<&NetworkTransform>,
    mut server: ResMut<RenetServer>,
) {
    for event in events.iter() {
        if let ServerEntityEvent::Spawned((entity, connection)) = event {
            let transform = match query.get(*entity) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let last_update = match transform.sent_updates.back() {
                Some(u) => u.clone(),
                None => continue,
            };

            let serialized = serialize_once(&TransformMessage::Update(last_update.clone()));
            server.send_message(connection.0, Channel::Transforms.id(), serialized);
        }
    }
}

/// Process acknowledgments from clients
fn handle_acks(
    mut query: Query<&mut NetworkTransform>,
    mut server: ResMut<RenetServer>,
    identities: Res<NetworkIdentities>,
    time: Res<Time>,
) {
    let seconds = time.raw_elapsed_seconds();
    'clients: for client_id in server.clients_id().into_iter() {
        while let Some(message) = server.receive_message(client_id, Channel::Transforms.id()) {
            let message: TransformMessage = match deserialize(&message) {
                Ok(m) => m,
                Err(_) => {
                    warn!(client_id, "Invalid transform message from client");
                    continue 'clients;
                }
            };
            match message {
                TransformMessage::Ack(ack) => {
                    let entity = match identities.get_entity(ack.identity) {
                        Some(e) => e,
                        None => {
                            warn!(
                                "Received transform ack for non-existent {:?} from {}",
                                ack.identity, client_id
                            );
                            continue;
                        }
                    };

                    let mut transform = match query.get_mut(entity) {
                        Ok(t) => t,
                        Err(_) => {
                            warn!("Received transform ack for entity without network transform {:?} from {}", entity, client_id);
                            continue;
                        }
                    };

                    let mut data = transform
                        .client_data
                        .entry(ConnectionId(client_id))
                        .or_default();
                    if data.last_sequence < ack.sequence_number {
                        data.last_sequence = ack.sequence_number;
                        data.last_ack = seconds;
                    }
                }
                _ => {
                    warn!("Received invalid transform message from {}", client_id);
                }
            }
        }
    }
}

/// Receives transform updates from the network
#[derive(Component, Default)]
pub struct NetworkedTransform {
    /// A series of transform updates
    buffered_updates: VecDeque<TransformUpdate>,
    /// How much to offset this transform from the accurate physics simulation.
    /// We reduce this value over time to smooth physics corrections.
    // TODO: Actually use this
    #[allow(dead_code)]
    visual_position_error: Option<Vec3>,
    had_next: bool,
    /// If this has ever been applied to a transform.
    /// Is `false` when newly created and set after the first update is applied.
    ever_applied: bool,
}

impl NetworkedTransform {
    /// Gets the relevant transform updates for the given tick
    fn relevant_updates(
        &mut self,
        tick: f32,
    ) -> Option<(&TransformUpdate, Option<&TransformUpdate>)> {
        // Find the next update to be interpolated to
        let next = self
            .buffered_updates
            .iter()
            .enumerate()
            .find(|(_, u)| u.sequence_number as f32 >= tick)
            .map(|(i, _)| i);
        let next = match next {
            Some(n) => n,
            None => {
                // Try to provide any update if never updated
                if !self.ever_applied {
                    return self.buffered_updates.front().map(|u| (u, None));
                }

                // No relevant update
                return None;
            }
        };

        let previous = if next > 0 { Some(next - 1) } else { None };
        // Remove updates before previous, we will never need them again
        if let Some(p) = previous {
            self.buffered_updates.drain(..p);
        }

        Some(match previous {
            Some(_) => (
                self.buffered_updates.get(1).unwrap(),
                self.buffered_updates.get(0),
            ),
            None => (self.buffered_updates.get(0).unwrap(), None),
        })
    }
}

const UPDATE_BUFFER_SIZE: usize = 150;
/// Stores transform updates that could not be applied
#[derive(Resource)]
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
    mut client: ResMut<RenetClient>,
    mut buffer: ResMut<BufferedTransformUpdates>,
    mut acknowledgments: Local<Vec<Acknowledgment>>,
) {
    while let Some(message) = client.receive_message(Channel::Transforms.id()) {
        let message: TransformMessage = match deserialize(&message) {
            Ok(m) => m,
            Err(_) => {
                warn!("Invalid transform message");
                continue;
            }
        };
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

    for ack in acknowledgments.drain(..) {
        client.send_message(
            Channel::Transforms.id(),
            serialize_once(&TransformMessage::Ack(ack)),
        );
    }
}

/// Apply the buffered transform messages to the relevant entities
fn apply_buffered_updates(
    mut buffer: ResMut<BufferedTransformUpdates>,
    mut query: Query<
        (Option<&mut NetworkedTransform>, Option<&ClientControlled>),
        With<NetworkIdentity>,
    >,
    identities: Res<NetworkIdentities>,
    mut unique_updates: Local<HashMap<NetworkIdentity, TransformUpdate>>,
    mut commands: Commands,
) {
    buffer.updates.retain(|update| {
        let entity = match identities.get_entity(update.identity) {
            Some(e) => e,
            None => return true,
        };

        let (networked, client_controlled) = match query.get_mut(entity) {
            Ok(n) => n,
            Err(_) => {
                return true;
            }
        };

        // TODO: Remove `if` once movement is server authoritative
        if client_controlled.is_none() {
            if let Some(mut networked) = networked {
                networked.buffered_updates.push_back(update.clone());
            } else {
                // Add networked transform component if not present
                let mut networked = NetworkedTransform::default();
                networked.buffered_updates.push_back(update.clone());
                commands
                    .entity(entity)
                    .insert((SpatialBundle::default(), networked));
            }
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
            }
            Entry::Vacant(v) => {
                v.insert(update);
            }
        }
    }
    buffer
        .updates
        .extend(unique_updates.drain().map(|(_, u)| u));
}

/// Applies transform updates to entities without physics simulation
fn sync_networked_transform(
    mut query: Query<(&mut NetworkedTransform, &mut Transform), Without<RigidBody>>,
    network_time: Res<ClientNetworkTime>,
) {
    let current_tick = network_time.interpolated_tick();
    for (mut networked, mut transform) in query.iter_mut() {
        let (next_update, previous_update) = match networked.relevant_updates(current_tick) {
            Some(u) => u,
            None => continue,
        };

        // Interpolate between updates if present
        let update = match previous_update {
            Some(previous) => next_update.interpolate(previous, current_tick),
            None => next_update.clone(),
        };

        if let Some(position) = update.position {
            transform.translation = position;
        }

        if let Some(rotation) = update.rotation {
            transform.rotation = rotation;
        }
    }
}

/// Applies transform updates to entities with physics
fn sync_networked_transform_physics(
    mut query: Query<
        (
            Entity,
            &mut NetworkedTransform,
            &mut Transform,
            Option<&mut Velocity>,
        ),
        With<RigidBody>,
    >,
    network_time: Res<ClientNetworkTime>,
    mut commands: Commands,
) {
    let current_tick = network_time.interpolated_tick();
    for (entity, mut networked_transform, mut transform, velocity) in query.iter_mut() {
        let (next_update, previous_update) =
            match networked_transform.relevant_updates(current_tick) {
                Some(u) => u,
                None => {
                    networked_transform.had_next = false;
                    continue;
                }
            };

        // Interpolate between updates if present
        let update = match previous_update {
            Some(previous) => previous.interpolate(next_update, current_tick),
            None => next_update.clone(),
        };

        if let Some(position) = update.position {
            transform.translation = position;
        }

        if let Some(rotation) = update.rotation {
            transform.rotation = rotation;
        }

        match velocity {
            Some(mut v) => {
                if let Some(linear_velocity) = update.linear_velocity {
                    v.linvel = linear_velocity;
                }

                if let Some(angular_velocity) = update.angular_velocity {
                    v.angvel = angular_velocity;
                }
            }
            None => {
                let velocity = Velocity {
                    linvel: update.linear_velocity.unwrap_or_default(),
                    angvel: update.angular_velocity.unwrap_or_default(),
                };
                commands.entity(entity).insert(velocity);
            }
        }

        networked_transform.ever_applied = true;
        networked_transform.had_next = true;
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
        app.register_type::<NetworkTransform>();

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.add_system_set(
                SystemSet::new()
                    .after(SpawningSystems::Spawn)
                    .with_system(update_transform.after(TimeSystem::Tick))
                    .with_system(handle_retransmission)
                    .with_system(handle_acks)
                    .with_system(handle_occasional_sync)
                    .with_system(handle_newly_spawned),
            );
        } else {
            app.init_resource::<BufferedTransformUpdates>()
                .add_system(
                    handle_transform_messages
                        .label(ClientTransformSystem::ReceiveMessages)
                        .with_run_criteria(run_if_client_connected),
                )
                .add_system_to_stage(
                    CoreStage::PostUpdate,
                    apply_buffered_updates
                        .label(ClientTransformSystem::ApplyBuffer)
                        .after(ClientTransformSystem::ReceiveMessages)
                        .after(SpawningSystems::Spawn),
                )
                .add_system_to_stage(
                    CoreStage::PostUpdate,
                    sync_networked_transform
                        .label(ClientTransformSystem::Sync)
                        .after(ClientTransformSystem::ApplyBuffer)
                        .after(TimeSystem::Interpolate)
                        .before(TransformSystem::TransformPropagate),
                )
                .add_system_to_stage(
                    CoreStage::PostUpdate,
                    sync_networked_transform_physics
                        .label(ClientTransformSystem::Sync)
                        .after(ClientTransformSystem::ApplyBuffer)
                        .after(TimeSystem::Interpolate)
                        .before(TransformSystem::TransformPropagate),
                );
        }
    }
}
