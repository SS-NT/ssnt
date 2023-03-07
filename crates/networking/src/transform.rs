use std::collections::VecDeque;

use bevy::{
    math::{Quat, Vec3},
    prelude::*,
    reflect::Reflect,
    transform::TransformSystem,
    utils::{hashbrown::hash_map::Entry, HashMap},
};
use bevy_rapier3d::prelude::{RigidBody, RigidBodyDisabled, Velocity};
use bevy_renet::{
    renet::{RenetClient, RenetServer},
    run_if_client_connected,
};
use physics::PhysicsEntityCommands;
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{deserialize, serialize_once, Channel},
    spawning::{ClientControlled, SpawningSystems},
    time::{ClientNetworkTime, ServerNetworkTime, TimeSystem},
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager,
};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Serialize, Deserialize)]
struct SequenceNumber(u32);

impl SequenceNumber {
    const fn from_tick(tick: u32) -> Self {
        Self(tick)
    }

    const fn as_tick(&self) -> f32 {
        self.0 as f32
    }

    fn between(from: Self, to: Self, tick: f32) -> f32 {
        let distance = to.0 - from.0;
        let t = (tick - from.as_tick()) / distance as f32;
        debug_assert!((0.0..=1.0).contains(&t));
        t
    }
}

/// The full state of a transform
#[derive(Clone, Copy)]
struct TransformSnapshot {
    sequence_number: SequenceNumber,
    position: Vec3,
    rotation: Quat,
    parent: Option<NetworkIdentity>,
    disabled: bool,
    physics: Option<PhysicsSnapshot>,
}

#[derive(Clone, Copy, Default)]
struct PhysicsSnapshot {
    linear_velocity: Vec3,
    angular_velocity: Vec3,
}

impl From<Velocity> for PhysicsSnapshot {
    fn from(value: Velocity) -> Self {
        Self {
            linear_velocity: value.linvel,
            angular_velocity: value.angvel,
        }
    }
}

impl TransformSnapshot {
    fn from_full(update: TransformUpdateData) -> Option<Self> {
        Some(Self {
            sequence_number: update.sequence_number,
            position: update.position?,
            rotation: update.rotation?,
            parent: update.parent?,
            disabled: update.disabled,
            physics: update.linear_velocity.zip(update.angular_velocity).map(
                |(linear_velocity, angular_velocity)| PhysicsSnapshot {
                    linear_velocity,
                    angular_velocity,
                },
            ),
        })
    }

    fn apply(&mut self, update: TransformUpdateData) {
        debug_assert_eq!(Some(self.sequence_number), update.delta_from);

        if let Some(position) = update.position {
            self.position = position;
        }
        if let Some(rotation) = update.rotation {
            self.rotation = rotation;
        }
        if let Some(linear_velocity) = update.linear_velocity {
            self.physics
                .get_or_insert_with(Default::default)
                .linear_velocity = linear_velocity;
        }
        if let Some(angular_velocity) = update.angular_velocity {
            self.physics
                .get_or_insert_with(Default::default)
                .angular_velocity = angular_velocity;
        }
        if let Some(parent) = update.parent {
            self.parent = parent;
        }
        self.disabled = update.disabled;

        self.sequence_number = update.sequence_number;
    }

    fn interpolate(from: &Self, to: &Self, tick: f32) -> Self {
        // Swap direction if necessary
        let mut from = from;
        let mut to = to;
        if from.sequence_number > to.sequence_number {
            std::mem::swap(&mut from, &mut to);
        }

        // Calculate at which time point we are between the updates
        let mut t = SequenceNumber::between(from.sequence_number, to.sequence_number, tick);

        // Do not interpolate if the parent changed
        if let Some(new_parent) = to.parent {
            if from.parent != Some(new_parent) {
                t = if t > 0.5 { 1.0 } else { 0.0 };
            }
        }

        let position = from.position.lerp(to.position, t);
        let rotation = from.rotation.lerp(to.rotation, t);
        let linear_velocity = interpolate_component(
            from.physics.map(|p| p.linear_velocity),
            to.physics.map(|p| p.linear_velocity),
            t,
            Vec3::lerp,
        );
        let angular_velocity = interpolate_component(
            from.physics.map(|p| p.angular_velocity),
            to.physics.map(|p| p.angular_velocity),
            t,
            Vec3::lerp,
        );
        let parent = if t > 0.5 { to.parent } else { from.parent };
        let frozen = if t > 0.5 { to.disabled } else { from.disabled };
        let physics =
            linear_velocity
                .zip(angular_velocity)
                .map(|(linear_velocity, angular_velocity)| PhysicsSnapshot {
                    angular_velocity,
                    linear_velocity,
                });

        Self {
            sequence_number: SequenceNumber::from_tick(tick as u32),
            position,
            rotation,
            parent,
            disabled: frozen,
            physics,
        }
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct Acknowledgment {
    identity: NetworkIdentity,
    sequence_number: SequenceNumber,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TransformUpdate {
    identity: NetworkIdentity,
    data: TransformUpdateData,
}

// TODO: Add delta compression
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct TransformUpdateData {
    sequence_number: SequenceNumber,
    /// The sequence number of the snapshot this update is based on
    delta_from: Option<SequenceNumber>,
    position: Option<Vec3>,
    rotation: Option<Quat>,
    linear_velocity: Option<Vec3>,
    angular_velocity: Option<Vec3>,
    parent: Option<Option<NetworkIdentity>>,
    disabled: bool,
}

impl TransformUpdateData {
    fn full(snapshot: TransformSnapshot) -> Self {
        Self {
            sequence_number: snapshot.sequence_number,
            delta_from: None,
            position: Some(snapshot.position),
            rotation: Some(snapshot.rotation),
            linear_velocity: snapshot.physics.map(|p| p.linear_velocity),
            angular_velocity: snapshot.physics.map(|p| p.angular_velocity),
            parent: Some(snapshot.parent),
            disabled: snapshot.disabled,
        }
    }

    fn diff(
        base: TransformSnapshot,
        new: TransformSnapshot,
        thresholds: Thresholds,
    ) -> Option<Self> {
        let update_position = !new
            .position
            .abs_diff_eq(base.position, thresholds.position_threshold);

        let update_rotation = !new
            .rotation
            .abs_diff_eq(base.rotation, thresholds.rotation_threshold);

        let update_parent = new.parent != base.parent;

        let update_frozen = new.disabled != base.disabled;

        if !update_position && !update_rotation && !update_parent && !update_frozen {
            return None;
        }

        Some(Self {
            sequence_number: new.sequence_number,
            delta_from: Some(base.sequence_number),
            position: update_position.then_some(new.position),
            rotation: update_rotation.then_some(new.rotation),
            linear_velocity: new
                .physics
                .and_then(|p| update_position.then_some(p.linear_velocity)),
            angular_velocity: new
                .physics
                .and_then(|p| update_rotation.then_some(p.angular_velocity)),
            parent: update_parent.then_some(new.parent),
            disabled: new.disabled,
        })
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
    /// The last time an ack was received
    last_ack: f32,
    // /// The sequence number we last sent this client
    // sent_sequence: Option<SequenceNumber>,
    /// The last sequence that was confirmed to have arrived
    acked_sequence: Option<SequenceNumber>,
    // /// The complete state the object was in at the last ack
    // acked_state: Option<TransformSnapshot>,
}

#[derive(Reflect, Clone, Copy)]
pub struct Thresholds {
    /// How much the position needs to move to be considered changed
    pub position_threshold: f32,
    /// How much the rotation needs to change to be considered changed
    pub rotation_threshold: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            position_threshold: 0.01,
            rotation_threshold: 0.01,
        }
    }
}

/// Sends transform changes to clients
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct NetworkTransform {
    /// How many times this transform is sent per second
    pub update_rate: f32,
    pub thresholds: Thresholds,
    /// How long to wait for a position ack before retransmitting.
    /// This is a multiplicator, 1 = 1 x RTT.
    /// Retransmission is only necessary when the update rate is below RTT or the transform has stopped moving.
    pub retransmission_multiplicator: f32,
    /// Every recorded state of the transform
    #[reflect(ignore)]
    snapshots: VecDeque<TransformSnapshot>,
    snapshots_to_keep: usize,
    #[reflect(ignore)]
    client_data: HashMap<ConnectionId, ClientData>,
    last_update: f32,
    last_change: f32,
}

impl Default for NetworkTransform {
    fn default() -> Self {
        Self {
            update_rate: 30.0,
            thresholds: Default::default(),
            retransmission_multiplicator: 2.0,
            snapshots: VecDeque::with_capacity(30),
            snapshots_to_keep: 30,
            client_data: Default::default(),
            last_update: Default::default(),
            last_change: Default::default(),
        }
    }
}

impl NetworkTransform {
    fn add_snapshot(&mut self, snapshot: TransformSnapshot) {
        if self.snapshots.len() >= self.snapshots_to_keep {
            self.snapshots.pop_front();
        }

        self.snapshots.push_back(snapshot);
    }
}

fn update_transform(
    mut query: Query<(
        &mut NetworkTransform,
        &Transform,
        &NetworkIdentity,
        Option<&Velocity>,
        Option<&Parent>,
        Option<&RigidBodyDisabled>,
    )>,
    identity_query: Query<&NetworkIdentity>,
    time: Res<Time>,
    visibilities: Res<NetworkVisibilities>,
    mut server: ResMut<RenetServer>,
    network_time: Res<ServerNetworkTime>,
) {
    let seconds = time.raw_elapsed_seconds();
    for (mut networked, transform, identity, velocity, parent, disabled_body) in query.iter_mut() {
        let networked: &mut NetworkTransform = &mut networked;

        // Respect update rate
        if networked.last_update + 1.0 / networked.update_rate > seconds {
            continue;
        }

        networked.last_update = seconds;

        let snapshot = TransformSnapshot {
            sequence_number: SequenceNumber::from_tick(network_time.current_tick()),
            position: transform.translation,
            rotation: transform.rotation,
            parent: parent
                .and_then(|p| identity_query.get(p.get()).ok())
                .copied(),
            disabled: disabled_body.is_some(),
            physics: velocity.map(|v| (*v).into()),
        };

        let last_snapshot = networked.snapshots.back();
        // TODO: We shouldn't construct an entire diff just to check if it changed
        if last_snapshot.is_none()
            || TransformUpdateData::diff(*last_snapshot.unwrap(), snapshot, networked.thresholds)
                .is_some()
        {
            networked.last_change = seconds;
        }

        networked.add_snapshot(snapshot);

        // Rarely send full update to recover from physics desync
        // let is_occasional_update = body.is_some() && networked.last_change + TRANSFORM_STILL_RESYNC_WAIT < seconds;

        let Some(visibility) = visibilities.visibility.get(identity) else {
            continue;
        };

        // TODO: We could group clients by their acked sequence
        for connection in visibility.observers() {
            let client_data = networked.client_data.entry(*connection).or_default();
            // Get the snapshot the client last acknowledged
            let base_snapshot = client_data.acked_sequence.and_then(|sequence| {
                networked
                    .snapshots
                    .iter()
                    .rev()
                    .find(|s| s.sequence_number == sequence)
                    .copied()
            });
            // Either create a diff or send a full copy
            let data = base_snapshot
                .map(|base| TransformUpdateData::diff(base, snapshot, networked.thresholds))
                .unwrap_or_else(|| Some(TransformUpdateData::full(snapshot)));
            let Some(data) = data else {
                // Transform did not significantly change
                continue;
            };
            let message = TransformMessage::Update(TransformUpdate {
                identity: *identity,
                data,
            });
            let serialized = serialize_once(&message);
            server.send_message(connection.0, Channel::Transforms.id(), serialized.clone());
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
                    if data.acked_sequence.is_none()
                        || data.acked_sequence.unwrap() < ack.sequence_number
                    {
                        data.acked_sequence = Some(ack.sequence_number);
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

/// How many transform snapshots a client keeps
const CLIENT_SNAPSHOT_BUFFER_SIZE: usize = 30;

/// Receives transform updates from the network
#[derive(Component, Default)]
pub struct NetworkedTransform {
    /// A series of transform snapshots
    snapshots: VecDeque<TransformSnapshot>,
    /// How much to offset this transform from the accurate physics simulation.
    /// We reduce this value over time to smooth physics corrections.
    // TODO: Actually use this
    #[allow(dead_code)]
    visual_position_error: Option<Vec3>,
    had_next: bool,
    /// If this has ever been applied to a transform.
    /// Is `false` when newly created and set after the first update is applied.
    ever_applied: bool,
    disabled: bool,
    /// The latest snapshot the server based it's updates on.
    /// This should never decrease.
    latest_base_sequence: Option<SequenceNumber>,
}

impl NetworkedTransform {
    fn add_snapshot(&mut self, snapshot: TransformSnapshot) {
        if self.snapshots.len() >= CLIENT_SNAPSHOT_BUFFER_SIZE {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(snapshot);
    }

    /// Gets the relevant transform snapshots for the given tick
    fn relevant_snapshots(
        &mut self,
        tick: f32,
    ) -> Option<(&TransformSnapshot, Option<&TransformSnapshot>)> {
        // Find the next snapshot to be interpolated to
        let next = self
            .snapshots
            .iter()
            .enumerate()
            .find(|(_, u)| u.sequence_number.as_tick() >= tick)
            .map(|(i, _)| i);
        let next = match next {
            Some(n) => n,
            None => {
                // Try to provide any update if never updated
                if !self.ever_applied {
                    return self.snapshots.back().map(|u| (u, None));
                }

                // No relevant update
                return None;
            }
        };

        let previous = if next > 0 { Some(next - 1) } else { None };

        Some((
            self.snapshots.get(next).unwrap(),
            previous.map(|p| self.snapshots.get(p).unwrap()),
        ))
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
                    sequence_number: update.data.sequence_number,
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
    mut query: Query<Option<&mut NetworkedTransform>, With<NetworkIdentity>>,
    identities: Res<NetworkIdentities>,
    mut unique_updates: Local<HashMap<NetworkIdentity, TransformUpdate>>,
    mut commands: Commands,
) {
    buffer.updates.retain(|update| {
        let entity = match identities.get_entity(update.identity) {
            Some(e) => e,
            None => return true,
        };

        let mut networked  = match query.get_mut(entity) {
            Ok(n) => n,
            Err(_) => {
                return true;
            }
        };

        let snapshot = if let Some(base_sequence) = update.data.delta_from {
            // Construct an updated snapshot from the base snapshot and the update
            if let Some(networked) = networked.as_mut() {
                if let Ok(index) = networked.snapshots.binary_search_by_key(&base_sequence, |snapshot| snapshot.sequence_number) {
                    networked.latest_base_sequence = Some(base_sequence);
                    let mut base_snapshot = networked.snapshots.get(index).cloned().unwrap();
                    base_snapshot.apply(update.data);
                    base_snapshot
                } else {
                    warn!("Received delta-compressed transform update and we don't have the original snapshot");
                    return false;
                }
            } else {
                warn!("Received delta-compressed transform update and client transform doesn't exist yet");
                return false;
            }
        } else {
            // Construct a snapshot from the full update
            let Some(snapshot) = TransformSnapshot::from_full(update.data) else {
                warn!("Received full transform with missing fields, this shouldn't happen");
                return false;
            };
            snapshot
        };

        if let Some(mut networked) = networked {
            networked.add_snapshot(snapshot);
        } else {
            // Add networked transform component if not present
            let mut networked = NetworkedTransform::default();
            networked.add_snapshot(snapshot);
            commands
                .entity(entity)
                .insert((SpatialBundle::default(), networked));
        }

        false
    });

    // Deduplicate the non-applied updates
    for update in buffer.updates.drain(..) {
        match unique_updates.entry(update.identity) {
            Entry::Occupied(mut o) => {
                let existing = o.get();
                // Replace if same identity and newer sequence number
                if existing.data.sequence_number < update.data.sequence_number {
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

/// Applies transform snapshots to entities without physics simulation
fn sync_networked_transform(
    mut query: Query<
        (&mut NetworkedTransform, &mut Transform),
        (Without<RigidBody>, Without<ClientControlled>),
    >,
    network_time: Res<ClientNetworkTime>,
) {
    let current_tick = network_time.interpolated_tick();
    for (mut networked, mut transform) in query.iter_mut() {
        let (next_snapshot, previous_snapshot) = match networked.relevant_snapshots(current_tick) {
            Some(u) => u,
            None => continue,
        };

        // Interpolate between snapshots if present
        let snapshot = match previous_snapshot {
            Some(previous_snapshot) => {
                TransformSnapshot::interpolate(previous_snapshot, next_snapshot, current_tick)
            }
            None => *next_snapshot,
        };

        transform.translation = snapshot.position;
        transform.rotation = snapshot.rotation;
    }
}

/// Applies transform updates to entities with physics
fn sync_networked_transform_physics(
    mut query: Query<(
        Entity,
        &mut NetworkedTransform,
        &mut Transform,
        Option<&mut Velocity>,
        Option<&Parent>,
        Option<&ClientControlled>,
    )>,
    identities: Res<NetworkIdentities>,
    network_time: Res<ClientNetworkTime>,
    mut commands: Commands,
) {
    let current_tick = network_time.interpolated_tick();
    for (entity, mut networked_transform, mut transform, velocity, parent, client_controlled) in
        query.iter_mut()
    {
        let (next_snapshot, previous_snapshot) =
            match networked_transform.relevant_snapshots(current_tick) {
                Some(u) => u,
                None => {
                    networked_transform.had_next = false;
                    continue;
                }
            };

        // Interpolate between snapshots if present
        let snapshot = match previous_snapshot {
            Some(previous_snapshot) => {
                TransformSnapshot::interpolate(previous_snapshot, next_snapshot, current_tick)
            }
            None => *next_snapshot,
        };

        if client_controlled.is_none() {
            transform.translation = snapshot.position;
            transform.rotation = snapshot.rotation;
        }

        if snapshot.parent != parent.and_then(|p| identities.get_identity(p.get())) {
            if let Some(parent) = snapshot.parent {
                if let Some(parent_entity) = identities.get_entity(parent) {
                    commands.entity(entity).set_parent(parent_entity);
                } else {
                    warn!(parent_id = ?parent, entity = ?entity, "Transform parent not found");
                }
            } else {
                commands.entity(entity).remove_parent();
            }
        }

        let disabled = snapshot.disabled;
        if disabled != networked_transform.disabled {
            commands.entity(entity).set_physics(!disabled);
            networked_transform.disabled = snapshot.disabled;
        }

        if client_controlled.is_none() {
            match velocity {
                Some(mut v) => {
                    if let Some(physics) = snapshot.physics {
                        v.linvel = physics.linear_velocity;
                        v.angvel = physics.angular_velocity;
                    }
                }
                None => {
                    let velocity = Velocity {
                        linvel: snapshot
                            .physics
                            .map(|p| p.linear_velocity)
                            .unwrap_or_default(),
                        angvel: snapshot
                            .physics
                            .map(|p| p.angular_velocity)
                            .unwrap_or_default(),
                    };
                    commands.entity(entity).insert(velocity);
                }
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
                    // .with_system(handle_retransmission)
                    .with_system(handle_acks),
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
