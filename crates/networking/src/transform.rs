use std::collections::VecDeque;

use crate::{self as networking, component::AppExt}; // This allows networking_derive to work in this crate itself
use bevy::{
    ecs::query::Has,
    math::{Quat, Vec3},
    platform::collections::{hash_map::Entry, HashMap, HashSet},
    prelude::*,
    reflect::{Reflect, TypePath},
};
use bevy_rapier3d::prelude::{
    Collider, CollisionGroups, ExternalForce, LockedAxes, QueryFilter, ReadRapierContext,
    RigidBody, RigidBodyDisabled, ShapeCastOptions, Velocity,
};
use bevy_renet::{RenetClient, RenetServer};
use networking_derive::Networked;
use physics::{ColliderGroup, PhysicsEntityCommands, SetPhysicsCommand, VisualError};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{deserialize, serialize_once, Channel},
    spawning::ClientControlled,
    time::{ClientNetworkTime, ServerNetworkTime},
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager, NetworkSet,
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
    /// Fully disabled (e.g. a stored item): rigid body + colliders off.
    disabled: bool,
    /// Attached and immobile (e.g. a limb): kinematic, colliders stay live.
    frozen: bool,
    physics: Option<PhysicsSnapshot>,
}

#[derive(Clone, Copy, Default)]
struct PhysicsSnapshot {
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    collider_group: ColliderGroup,
    locked_vertical: bool,
}

impl PhysicsSnapshot {
    /// Whether the body moves fast enough to warrant client-side simulation.
    fn is_moving(&self) -> bool {
        self.linear_velocity.length_squared() > GAP_SIMULATE_MIN_SPEED.powi(2)
            || self.angular_velocity.length_squared() > GAP_SIMULATE_MIN_ANGULAR.powi(2)
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
            frozen: update.frozen,
            physics: update.linear_velocity.zip(update.angular_velocity).map(
                |(linear_velocity, angular_velocity)| PhysicsSnapshot {
                    linear_velocity,
                    angular_velocity,
                    collider_group: update.collider_group.unwrap_or_default(),
                    locked_vertical: update.locked_vertical,
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
        if let Some(physics) = &mut self.physics {
            physics.locked_vertical = update.locked_vertical;
        }
        self.disabled = update.disabled;
        self.frozen = update.frozen;

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
        let disabled = if t > 0.5 { to.disabled } else { from.disabled };
        let frozen = if t > 0.5 { to.frozen } else { from.frozen };
        let physics =
            linear_velocity
                .zip(angular_velocity)
                .map(|(linear_velocity, angular_velocity)| PhysicsSnapshot {
                    angular_velocity,
                    linear_velocity,
                    collider_group: to.physics.map(|p| p.collider_group).unwrap_or_default(),
                    locked_vertical: to.physics.map(|p| p.locked_vertical).unwrap_or_default(),
                });

        Self {
            sequence_number: SequenceNumber::from_tick(tick as u32),
            position,
            rotation,
            parent,
            disabled,
            frozen,
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
    collider_group: Option<ColliderGroup>,
    locked_vertical: bool,
    parent: Option<Option<NetworkIdentity>>,
    disabled: bool,
    frozen: bool,
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
            collider_group: snapshot.physics.map(|p| p.collider_group),
            locked_vertical: snapshot
                .physics
                .map(|p| p.locked_vertical)
                .unwrap_or_default(),
            parent: Some(snapshot.parent),
            disabled: snapshot.disabled,
            frozen: snapshot.frozen,
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

        let update_disabled = new.disabled != base.disabled;
        let update_frozen = new.frozen != base.frozen;

        let update_locked =
            new.physics.map(|p| p.locked_vertical) != base.physics.map(|p| p.locked_vertical);

        let update_collider =
            new.physics.map(|p| p.collider_group) != base.physics.map(|p| p.collider_group);

        // Send once when the body crosses the moving/resting boundary so the client learns it
        // came to rest (and stops simulating it) even while position stays below threshold.
        let was_moving = base.physics.map(|p| p.is_moving()).unwrap_or(false);
        let now_moving = new.physics.map(|p| p.is_moving()).unwrap_or(false);
        let update_velocity = was_moving != now_moving;

        if !update_position
            && !update_rotation
            && !update_parent
            && !update_disabled
            && !update_frozen
            && !update_collider
            && !update_locked
            && !update_velocity
        {
            return None;
        }

        Some(Self {
            sequence_number: new.sequence_number,
            delta_from: Some(base.sequence_number),
            position: update_position.then_some(new.position),
            rotation: update_rotation.then_some(new.rotation),
            linear_velocity: new
                .physics
                .and_then(|p| (update_position || update_velocity).then_some(p.linear_velocity)),
            angular_velocity: new
                .physics
                .and_then(|p| (update_rotation || update_velocity).then_some(p.angular_velocity)),
            collider_group: new
                .physics
                .and_then(|p| update_collider.then_some(p.collider_group)),
            locked_vertical: new.physics.map(|p| p.locked_vertical).unwrap_or_default(),
            parent: update_parent.then_some(new.parent),
            disabled: new.disabled,
            frozen: new.frozen,
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
            position_threshold: 0.005,
            rotation_threshold: 0.005,
        }
    }
}

/// Sends transform changes to clients
#[derive(Component, Reflect)]
#[reflect(Component, Default)]
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
        Entity,
        &mut NetworkTransform,
        &Transform,
        &NetworkIdentity,
        Option<&CollisionGroups>,
        Option<&LockedAxes>,
        Option<&Velocity>,
        Option<&ChildOf>,
        Has<RigidBody>,
        Has<RigidBodyDisabled>,
        Has<physics::Frozen>,
    )>,
    identity_query: Query<&NetworkIdentity>,
    time: Res<Time>,
    visibilities: Res<NetworkVisibilities>,
    mut server: ResMut<RenetServer>,
    network_time: Res<ServerNetworkTime>,
    mut commands: Commands,
) {
    let seconds = time.elapsed_secs();
    let locked_rotation_vertical = LockedAxes::ROTATION_LOCKED_X | LockedAxes::ROTATION_LOCKED_Z;
    for (
        entity,
        mut networked,
        transform,
        identity,
        collision_group,
        locked_axes,
        velocity,
        parent,
        has_body,
        body_disabled,
        frozen,
    ) in query.iter_mut()
    {
        let networked: &mut NetworkTransform = &mut networked;

        // Respect update rate
        if networked.last_update + 1.0 / networked.update_rate > seconds {
            continue;
        }

        networked.last_update = seconds;

        // Insert velocity component so we can synchronize it
        if has_body && velocity.is_none() {
            commands.entity(entity).insert(Velocity::default());
        }

        let snapshot = TransformSnapshot {
            sequence_number: SequenceNumber::from_tick(network_time.current_tick()),
            position: transform.translation,
            rotation: transform.rotation,
            parent: parent
                .and_then(|p| identity_query.get(p.parent()).ok())
                .copied(),
            disabled: body_disabled,
            frozen,
            physics: velocity.map(|v| PhysicsSnapshot {
                linear_velocity: v.linear,
                angular_velocity: v.angular,
                collider_group: collision_group
                    .and_then(|c| (*c).try_into().ok())
                    .unwrap_or_default(),
                locked_vertical: locked_axes
                    .map(|axes| *axes & locked_rotation_vertical == locked_rotation_vertical)
                    .unwrap_or_default(),
            }),
        };

        let last_snapshot = networked.snapshots.back();
        // TODO: We shouldn't construct an entire diff just to check if it changed
        let changed = last_snapshot.is_none()
            || TransformUpdateData::diff(*last_snapshot.unwrap(), snapshot, networked.thresholds)
                .is_some();

        // Only record a snapshot when something actually changed
        if changed {
            networked.last_change = seconds;
            networked.add_snapshot(snapshot);
        }

        let snapshot = *networked.snapshots.back().unwrap();

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
    let seconds = time.elapsed_secs();
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

                    let data = transform
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
/// How long a client will extrapolate an object before freezing it at its last position
const CLIENT_MAX_PHYSICS_EXTRAPOLATION_TICKS: f32 = 15.0;
/// Linear speed below which a body counts as at rest: it just holds its pose kinematically.
const GAP_SIMULATE_MIN_SPEED: f32 = 0.1;
/// Angular speed (rad/s) counterpart to [`GAP_SIMULATE_MIN_SPEED`].
const GAP_SIMULATE_MIN_ANGULAR: f32 = 0.1;
/// How large a snapshot gap (in server ticks) must be before handing a moving body to
/// the physics engine to simulate.
const GAP_SIMULATE_MIN_TICKS: f32 = 4.0;
/// A snapshot gap (in server ticks) beyond which interpolating between two snapshots would
/// snap the object toward the newer one, because the interpolation parameter already starts
/// near the end. Past this we jump to the new pose and let `VisualError` ease it in.
const MAX_SMOOTH_INTERPOLATION_GAP_TICKS: f32 = 2.0 * CLIENT_MAX_PHYSICS_EXTRAPOLATION_TICKS;
/// Correction distance (metres) above which we snap instead of easing: a jump this large is a
/// teleport, not a misprediction or a rested object, so sliding to it looks wrong.
const TELEPORT_SNAP_DISTANCE: f32 = 4.0;
/// Speed (m/s) below which a controlled body counts as standing still and will not be checked.
const PREDICT_MIN_SPEED: f32 = 0.05;
/// Applied movement force above which we sweep along the force direction even when velocity is
/// low. Without this a player pressing into a kinematic object has ~0 velocity (it blocks them),
/// so the object never wakes and the player is stuck against it.
const PREDICT_MIN_FORCE: f32 = 0.1;
/// Nominal probe speed (m/s) used for the force-driven sweep when blocked, giving a short fixed
/// lookahead (`PREDICT_PROBE_SPEED * PREDICT_WAKE_LEAD_SECONDS` metres) in the push direction.
const PREDICT_PROBE_SPEED: f32 = 1.5;
/// How far ahead (seconds of travel) the wake shape-cast looks: the swept distance is the
/// body's speed times this, i.e. where it will be after this lead window.
const PREDICT_WAKE_LEAD_SECONDS: f32 = 0.25;
/// Slack (metres) added to the swept collider, so an object starts being predicted before actually hitting.
const PREDICT_WAKE_MARGIN: f32 = 0.15;
/// Distance (metres) from a player within which an incoming moving object is worth shape-casting
const PREDICT_INCOMING_CULL_DISTANCE: f32 = 2.5;
/// Ticks added on top of RTT/2 before an incoming snapshot is trusted to reflect our push and
/// end prediction. Covers the movement upload plus a tick or two of server reaction.
// TODO: Switch out with confirmation of received input from server
const RECONCILE_MARGIN_TICKS: f32 = 2.0;
/// Seconds after the last controlled body leaves the wake radius before a predicted body may
/// hand back to interpolation.
const PREDICT_HYSTERESIS_SECONDS: f32 = 0.15;
/// Seconds past prediction start after which we give up waiting for a confirming snapshot and
/// hand back anyway (the server never registered a push).
const PREDICT_TIMEOUT_SECONDS: f32 = 1.0;

/// The physics body mode we drive a remote object with on the client.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClientBody {
    /// Follows interpolated snapshots exactly.
    Kinematic,
    /// Free-simulating to extrapolate a snapshot gap.
    Dynamic,
    /// Free-simulating to predict the result of a local player interaction, until the server
    /// has simulated past the moment we began predicting. Snapshots are not applied meanwhile.
    Predicting,
}

impl ClientBody {
    /// Whether rapier owns the body (we don't overwrite it from snapshots).
    fn is_simulated(self) -> bool {
        matches!(self, ClientBody::Dynamic | ClientBody::Predicting)
    }
}

/// Receives transform updates from the network
#[derive(Component, Default)]
pub struct NetworkedTransform {
    /// A series of transform snapshots
    snapshots: VecDeque<TransformSnapshot>,
    had_next: bool,
    /// If this has ever been applied to a transform.
    /// Is `false` when newly created and set after the first update is applied.
    ever_applied: bool,
    disabled: bool,
    frozen: bool,
    locked_vertical: bool,
    collider_group: ColliderGroup,
    /// The latest snapshot the server based it's updates on.
    /// This should never decrease.
    latest_base_sequence: Option<SequenceNumber>,
    /// The body mode we last set for client-side interpolation/extrapolation.
    client_body: Option<ClientBody>,
    /// Server-tick estimate at which interaction prediction began. Snapshots at or after this
    /// are trusted to reflect our push and drive reconciliation.
    predict_start_tick: Option<f32>,
    /// Elapsed seconds the promotion pass last found a controlled body within the wake radius.
    last_woken: f32,
    /// Wall-clock time (elapsed seconds) the most recent snapshot was received.
    last_received: f32,
}

impl NetworkedTransform {
    pub fn is_simulating(&self) -> Option<bool> {
        self.client_body.map(ClientBody::is_simulated)
    }

    /// Wall-clock time (elapsed seconds) the most recent snapshot was received.
    pub fn last_received(&self) -> f32 {
        self.last_received
    }

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
                if let Some(last_snapshot) = self.snapshots.back() {
                    // Try to provide any update if never updated or last update is too old to extrapolate
                    if !self.ever_applied
                        || tick - last_snapshot.sequence_number.as_tick()
                            > CLIENT_MAX_PHYSICS_EXTRAPOLATION_TICKS
                    {
                        return Some((last_snapshot, None));
                    }
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

/// Marker component for entities allowing movement to be sent from clients.
#[derive(Component, TypePath, Networked)]
#[networked(client = "ClientMovementClient")]
pub struct ClientMovement;

#[derive(Component, Default, TypePath, Networked)]
#[networked(server = "ClientMovement")]
pub struct ClientMovementClient;

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
    time: Res<Time>,
    mut unique_updates: Local<HashMap<NetworkIdentity, TransformUpdate>>,
    mut commands: Commands,
) {
    let seconds = time.elapsed_secs();
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
            networked.last_received = seconds;
        } else {
            // Add networked transform component if not present
            let mut networked = NetworkedTransform::default();
            networked.add_snapshot(snapshot);
            networked.last_received = seconds;
            commands
                .entity(entity)
                .insert((Transform::default(), Visibility::default(), networked));
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

/// The representative collider of a body: `root` itself if it carries one, else the nearest
/// descendant collider.
fn representative_collider(
    root: Entity,
    children: &Query<&Children>,
    colliders: &Query<(&Collider, &GlobalTransform)>,
    bodies: &Query<(), With<RigidBody>>,
) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if colliders.contains(entity) {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            for child in kids.iter() {
                // A nested body owns the colliders below it; don't descend into it.
                if !bodies.contains(child) {
                    stack.push(child);
                }
            }
        }
    }
    None
}

/// Climbs from a collider to the first ancestor (inclusive) satisfying `belongs`.
fn owning_body(
    collider: Entity,
    parents: &Query<&ChildOf>,
    mut belongs: impl FnMut(Entity) -> bool,
) -> Option<Entity> {
    let mut entity = collider;
    loop {
        if belongs(entity) {
            return Some(entity);
        }
        match parents.get(entity) {
            Ok(child_of) => entity = child_of.parent(),
            Err(_) => return None,
        }
    }
}

/// Promotes networked objects a controlled body is about to touch to dynamic bodies so a push
/// can be predicted before contact, and seeds them with the object's estimated present-time
/// state so the prediction lines up with where the server has the object *now* rather than
/// where it was rendered (interpolation runs behind the server). The visual discontinuity of
/// that jump is pushed into `VisualError`, which eases it out.
#[allow(clippy::too_many_arguments)]
fn predict_interacted_objects(
    players: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&Velocity>,
            Option<&ExternalForce>,
        ),
        With<ClientControlled>,
    >,
    mut objects: Query<
        (
            Entity,
            &mut NetworkedTransform,
            &mut Transform,
            Option<&mut Velocity>,
            Option<&mut VisualError>,
        ),
        Without<ClientControlled>,
    >,
    colliders: Query<(&Collider, &GlobalTransform)>,
    child_query: Query<&Children>,
    parents: Query<&ChildOf>,
    bodies: Query<(), With<RigidBody>>,
    is_player: Query<(), With<ClientControlled>>,
    rapier: ReadRapierContext,
    network_time: Res<ClientNetworkTime>,
    time: Res<Time>,
    mut to_wake: Local<HashSet<Entity>>,
    mut commands: Commands,
) {
    to_wake.clear();
    let Ok(context) = rapier.single() else {
        return;
    };

    let options = ShapeCastOptions {
        max_time_of_impact: PREDICT_WAKE_LEAD_SECONDS,
        target_distance: PREDICT_WAKE_MARGIN,
        // Only register contacts we're closing on, so brushing/resting alongside wakes nothing.
        stop_at_penetration: false,
        compute_impact_geometry_on_penetration: false,
    };

    // Pass A: each moving player sweeps its own collider forward along its velocity. The swept
    // distance is `speed * PREDICT_WAKE_LEAD_SECONDS`, so it looks exactly as far ahead as the
    // player will travel in the lead window.
    let mut player_positions: Vec<Vec3> = Vec::new();
    for (player_root, global, velocity, force) in players.iter() {
        player_positions.push(global.translation());

        // Sweep along actual velocity when moving; when blocked fall back to the applied
        // movement force so pushing into it still wakes it.
        let velocity = velocity.map(|v| v.linear).unwrap_or(Vec3::ZERO);
        let sweep = if velocity.length() >= PREDICT_MIN_SPEED {
            velocity
        } else {
            match force.map(|f| f.force) {
                Some(force) if force.length() >= PREDICT_MIN_FORCE => {
                    force.normalize() * PREDICT_PROBE_SPEED
                }
                _ => continue,
            }
        };
        let Some(collider_entity) =
            representative_collider(player_root, &child_query, &colliders, &bodies)
        else {
            continue;
        };
        let (collider, collider_transform) = colliders.get(collider_entity).unwrap();
        let (_, rotation, translation) = collider_transform.to_scale_rotation_translation();
        let filter = QueryFilter::default().exclude_rigid_body(player_root);
        if let Some((hit, _)) = context.cast_shape(
            translation,
            rotation,
            sweep,
            &*collider.raw,
            options,
            filter,
        ) {
            if let Some(object) = owning_body(hit, &parents, |e| objects.contains(e)) {
                to_wake.insert(object);
            }
        }
    }

    // Pass B: each moving networked object sweeps its collider along its velocity; wake it if it
    // would run into a player. This catches objects thrown/sliding at a stationary player.
    for (object_root, networked, ..) in objects.iter() {
        if networked.client_body != Some(ClientBody::Kinematic)
            || networked.disabled
            || networked.frozen
        {
            continue;
        }
        let Some(velocity) = networked
            .snapshots
            .back()
            .and_then(|s| s.physics)
            .filter(|p| p.is_moving())
            .map(|p| p.linear_velocity)
        else {
            continue;
        };
        let Some(collider_entity) =
            representative_collider(object_root, &child_query, &colliders, &bodies)
        else {
            continue;
        };
        let (collider, collider_transform) = colliders.get(collider_entity).unwrap();
        let (_, rotation, translation) = collider_transform.to_scale_rotation_translation();
        // Cheap cull: ignore objects nowhere near a player before doing the shape cast.
        if !player_positions
            .iter()
            .any(|p| p.distance_squared(translation) < PREDICT_INCOMING_CULL_DISTANCE.powi(2))
        {
            continue;
        }
        let filter = QueryFilter::default().exclude_rigid_body(object_root);
        if let Some((hit, _)) = context.cast_shape(
            translation,
            rotation,
            velocity,
            &*collider.raw,
            options,
            filter,
        ) {
            if owning_body(hit, &parents, |e| is_player.contains(e)).is_some() {
                to_wake.insert(object_root);
            }
        }
    }

    let seconds = time.elapsed_secs();
    let (Some(tick_seconds), Some(present_tick)) = (
        network_time.server_tick_seconds,
        network_time.estimated_server_tick(seconds),
    ) else {
        return;
    };

    for &entity in to_wake.iter() {
        let Ok((_, mut networked, mut transform, velocity, visual_error)) = objects.get_mut(entity)
        else {
            continue;
        };

        // Keep the hysteresis timer fresh while the player lingers, whether or not we promote.
        networked.last_woken = seconds;

        // Only promote objects we're currently interpolating kinematically; leave controlled,
        // disabled, frozen, gap-extrapolating or already-predicting bodies alone.
        if networked.client_body != Some(ClientBody::Kinematic)
            || networked.disabled
            || networked.frozen
        {
            continue;
        }

        let Some(last) = networked.snapshots.back().copied() else {
            continue;
        };

        // Only bodies the server actually simulates should be predicted, not static objects.
        if last.physics.is_none() {
            continue;
        }

        // Extrapolate the latest snapshot to the estimated present server tick.
        let dt = (present_tick - last.sequence_number.as_tick()).max(0.0) * tick_seconds;
        let (linear, angular) = last
            .physics
            .map(|p| (p.linear_velocity, p.angular_velocity))
            .unwrap_or_default();
        let present_translation = last.position + linear * dt;
        let present_rotation = (Quat::from_scaled_axis(angular * dt) * last.rotation).normalize();

        // Preserve the currently-rendered pose as a visual offset so the jump to present is
        // invisible and eases out instead of popping.
        let offset_translation = transform.translation - present_translation;
        let offset_rotation = transform.rotation * present_rotation.inverse();
        transform.translation = present_translation;
        transform.rotation = present_rotation;
        match visual_error {
            Some(mut error) => error.add(offset_translation, offset_rotation),
            None => {
                let mut error = VisualError::default();
                error.add(offset_translation, offset_rotation);
                commands.entity(entity).insert(error);
            }
        }

        match velocity {
            Some(mut v) => {
                v.linear = linear;
                v.angular = angular;
            }
            None => {
                commands.entity(entity).insert(Velocity { linear, angular });
            }
        }

        commands.entity(entity).insert(RigidBody::Dynamic);
        networked.client_body = Some(ClientBody::Predicting);
        networked.predict_start_tick = Some(present_tick);
    }
}

/// Applies transform updates to entities with physics
fn sync_networked_transform_physics(
    mut query: Query<(
        Entity,
        &mut NetworkedTransform,
        &mut Transform,
        Option<&mut Velocity>,
        Option<&ChildOf>,
        Option<&mut LockedAxes>,
        Option<Ref<ClientMovementClient>>,
        Has<ClientControlled>,
        Option<&mut VisualError>,
        Option<&RigidBody>,
    )>,
    identities: Res<NetworkIdentities>,
    network_time: Res<ClientNetworkTime>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let current_tick = network_time.interpolated_tick();
    let seconds = time.elapsed_secs();
    let tick_seconds = network_time.server_tick_seconds;
    let present_tick = network_time.estimated_server_tick(seconds);
    for (
        entity,
        mut networked_transform,
        mut transform,
        velocity,
        parent,
        locked_axes,
        client_movement,
        controlled,
        visual_error,
        rigid_body,
    ) in query.iter_mut()
    {
        let was_simulating = networked_transform
            .client_body
            .map(ClientBody::is_simulated)
            .unwrap_or(false);

        // Interaction prediction: rapier owns the body, so we don't apply snapshots until the
        // server has simulated past the moment we began predicting (or we give up).
        if networked_transform.client_body == Some(ClientBody::Predicting) {
            let start = networked_transform
                .predict_start_tick
                .unwrap_or(current_tick);
            let margin =
                network_time.round_trip_ticks().unwrap_or(0.0) / 2.0 + RECONCILE_MARGIN_TICKS;
            let confirmed = networked_transform
                .snapshots
                .back()
                .map(|s| s.sequence_number.as_tick() >= start + margin)
                .unwrap_or(false);
            let still_woken = seconds - networked_transform.last_woken < PREDICT_HYSTERESIS_SECONDS;
            let timed_out = match (present_tick, tick_seconds) {
                (Some(present), Some(ts)) => (present - start) * ts > PREDICT_TIMEOUT_SECONDS,
                _ => false,
            };
            // If the server disabled/froze/stored the object, abort and let the normal path apply it.
            let interrupted = networked_transform
                .snapshots
                .back()
                .map(|s| s.disabled || s.frozen)
                .unwrap_or(false);

            if !((confirmed && !still_woken) || timed_out || interrupted) {
                networked_transform.had_next = false;
                networked_transform.ever_applied = true;
                continue;
            }

            commands
                .entity(entity)
                .insert(RigidBody::KinematicPositionBased);
            networked_transform.client_body = Some(ClientBody::Kinematic);
            networked_transform.predict_start_tick = None;
        }

        let (next_snapshot, previous_snapshot) =
            match networked_transform.relevant_snapshots(current_tick) {
                Some(u) => u,
                None => {
                    // No snapshot ahead of us, we might want to simulate the object
                    let last = networked_transform.snapshots.back();
                    // We might have a tick or two without already having a snapshot,
                    // we don't want to simulate such a short interruption
                    let gap = last
                        .map(|s| current_tick - s.sequence_number.as_tick())
                        .unwrap_or(0.0);
                    // Non-moving objects don't need to be simulated
                    let moving = last
                        .and_then(|s| s.physics)
                        .map(|p| p.is_moving())
                        .unwrap_or(false);
                    if gap > GAP_SIMULATE_MIN_TICKS
                        && moving
                        && !controlled
                        && !networked_transform.disabled
                        && !networked_transform.frozen
                        && networked_transform.client_body != Some(ClientBody::Dynamic)
                    {
                        commands.entity(entity).insert(RigidBody::Dynamic);
                        networked_transform.client_body = Some(ClientBody::Dynamic);
                    }
                    networked_transform.had_next = false;
                    continue;
                }
            };

        // Measure the gap between the two snapshots we have: large gaps need smoothing
        // (resting object or dropped packets).
        let interpolation_gap = previous_snapshot
            .map(|p| next_snapshot.sequence_number.as_tick() - p.sequence_number.as_tick())
            .unwrap_or(0.0);

        // Interpolate between snapshots if present
        let snapshot = match previous_snapshot {
            Some(previous_snapshot) => {
                TransformSnapshot::interpolate(previous_snapshot, next_snapshot, current_tick)
            }
            None => *next_snapshot,
        };

        let ignore_position =
            controlled && client_movement.map(|m| !m.is_added()).unwrap_or_default();
        let parent_changed =
            snapshot.parent != parent.and_then(|p| identities.get_identity(p.parent()));
        // Smooth discontinuities instead of snapping: either the client simulated the body and
        // must reconcile with the server, or the object rested long enough that the snapshots
        // we interpolate between are far apart.
        let resynced_after_simulation =
            networked_transform.ever_applied && !networked_transform.had_next && was_simulating;
        let resynced_after_rest = networked_transform.ever_applied
            && interpolation_gap > MAX_SMOOTH_INTERPOLATION_GAP_TICKS;
        if !ignore_position {
            let position_error = transform.translation - snapshot.position;
            // Only ease in a plausible correction; a large jump is a teleport, snap to it.
            let is_teleport = position_error.length_squared() > TELEPORT_SNAP_DISTANCE.powi(2);
            if (resynced_after_simulation || resynced_after_rest) && !parent_changed && !is_teleport
            {
                let rotation_error = transform.rotation * snapshot.rotation.inverse();
                match visual_error {
                    Some(mut error) => error.add(position_error, rotation_error),
                    None => {
                        let mut error = VisualError::default();
                        error.add(position_error, rotation_error);
                        commands.entity(entity).insert(error);
                    }
                }
            }
            transform.translation = snapshot.position;
            transform.rotation = snapshot.rotation;
        }

        if parent_changed {
            if let Some(parent) = snapshot.parent {
                if let Some(parent_entity) = identities.get_entity(parent) {
                    commands.entity(entity).insert(ChildOf(parent_entity));
                } else {
                    warn!(parent_id = ?parent, entity = ?entity, "Transform parent not found");
                }
            } else {
                commands.entity(entity).remove::<ChildOf>();
            }
        }

        let disabled = snapshot.disabled;
        let frozen = snapshot.frozen;
        let was_frozen = networked_transform.frozen;
        let disabled_changed = disabled != networked_transform.disabled;
        let frozen_changed = frozen != was_frozen;
        let collider_group_changed = snapshot
            .physics
            .map(|p| p.collider_group != networked_transform.collider_group)
            .unwrap_or_default();
        if disabled_changed || frozen_changed || collider_group_changed {
            let new_group = collider_group_changed
                .then(|| snapshot.physics.map(|p| p.collider_group))
                .flatten();
            if frozen {
                commands.entity(entity).freeze(new_group);
            } else if disabled {
                commands.queue(SetPhysicsCommand {
                    entity,
                    enabled: false,
                    disable_colliders: true,
                    new_group,
                });
            } else if was_frozen {
                commands.entity(entity).unfreeze(new_group);
            } else {
                commands.queue(SetPhysicsCommand {
                    entity,
                    enabled: true,
                    disable_colliders: true,
                    new_group,
                });
            }
            networked_transform.disabled = disabled;
            networked_transform.frozen = frozen;
            if let Some(group) = snapshot.physics.map(|p| p.collider_group) {
                networked_transform.collider_group = group;
            }
            if disabled_changed || frozen_changed {
                networked_transform.client_body = None;
            }
        }

        // Remote objects follow snapshots kinematically; we don't simulate them while interpolating.
        if !controlled
            && !disabled
            && !frozen
            && rigid_body.copied() != Some(RigidBody::KinematicPositionBased)
        {
            commands
                .entity(entity)
                .insert(RigidBody::KinematicPositionBased);
            networked_transform.client_body = Some(ClientBody::Kinematic);
        }

        // Update rotation lock
        if let Some(physics) = &snapshot.physics {
            let locked_vertical = physics.locked_vertical;
            if locked_vertical != networked_transform.locked_vertical {
                networked_transform.locked_vertical = locked_vertical;
                let rotation_lock = LockedAxes::ROTATION_LOCKED_X | LockedAxes::ROTATION_LOCKED_Z;
                match (locked_axes, locked_vertical) {
                    (Some(mut axes), true) => *axes |= rotation_lock,
                    (Some(mut axes), false) => *axes &= !rotation_lock,
                    (None, true) => {
                        commands.entity(entity).insert(rotation_lock);
                    }
                    _ => {}
                }
            }
        }

        if !ignore_position {
            match velocity {
                Some(mut v) => {
                    if let Some(physics) = snapshot.physics {
                        v.linear = physics.linear_velocity;
                        v.angular = physics.angular_velocity;
                    }
                }
                None => {
                    let velocity = Velocity {
                        linear: snapshot
                            .physics
                            .map(|p| p.linear_velocity)
                            .unwrap_or_default(),
                        angular: snapshot
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

pub(crate) struct TransformPlugin;

impl Plugin for TransformPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<NetworkTransform>()
            .add_networked_component::<ClientMovement, ClientMovementClient>();

        if app
            .world()
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.add_systems(
                PostUpdate,
                (
                    handle_acks,
                    update_transform.after(bevy_rapier3d::plugin::PhysicsSet::Writeback),
                    // TODO: Write outgoing messages again
                )
                    .chain()
                    .in_set(NetworkSet::ServerSyncPhysics),
            );
        } else {
            app.init_resource::<BufferedTransformUpdates>().add_systems(
                PreUpdate,
                (
                    handle_transform_messages,
                    apply_buffered_updates,
                    sync_networked_transform,
                    predict_interacted_objects,
                    sync_networked_transform_physics,
                )
                    .chain()
                    .in_set(NetworkSet::ClientApply),
            );
        }
    }
}
