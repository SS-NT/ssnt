use std::{
    borrow::Cow,
    clone::Clone,
    marker::PhantomData,
    num::NonZeroU32,
    ops::{Deref, DerefMut},
};

use bevy::{
    ecs::system::{SystemParam, SystemParamFetch},
    prelude::*,
    reflect::TypeUuid,
    scene::{InstanceId, SceneInstance},
    utils::{HashSet, Uuid},
};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{
        AppExt as MessagingAppExt, MessageEvent, MessageReceivers, MessageSender, MessagingSystem,
    },
    spawning::SpawningSystems,
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager, NetworkSystem,
};

pub use bytes::{Buf, BufMut, Bytes, BytesMut};
// TODO: Replace with handy method
pub use bincode::options as serializer_options;
pub use bincode::Deserializer as ComponentDeserializer;
pub use bincode::Serializer as ComponentSerializer;

/// A trait implemented by any component that should be networked to clients.
pub trait NetworkedToClient: Component {
    type Param: SystemParam;

    /// Does this component serialize differently depending on who the receiver is?
    fn receiver_matters() -> bool;

    /// Serialize this component to send it over the network.
    /// Returns `None` if it should not be sent to the given receiver.
    /// # Arguments
    ///
    /// * `since_tick` - The tick to diff from. Is None if the full state should be serialized.
    ///
    fn serialize<'w, 's>(
        &mut self,
        param: &<<Self::Param as SystemParam>::Fetch as SystemParamFetch<'w, 's>>::Item,
        receiver: Option<ConnectionId>,
        since_tick: Option<NonZeroU32>,
    ) -> Option<Bytes>;

    // TODO: Add is_changed so we can efficiently use change detection for updates
}

/// A trait implemented by any component that receives network updates from the server.
pub trait NetworkedFromServer: Component + TypeUuid {
    type Param: SystemParam;

    fn deserialize<'w, 's>(
        &mut self,
        param: &<<Self::Param as SystemParam>::Fetch as SystemParamFetch<'w, 's>>::Item,
        data: &[u8],
    );

    /// The initial value used if the component is not already present.
    /// Returns `None` if the component should not be added automatically.
    fn default_if_missing() -> Option<Box<Self>>;
}

/// A variable that is networked to clients.
#[derive(Default)]
pub struct NetworkVar<T> {
    value: T,
    /// The value before the last change to `value`.
    /// Used to diff the most recent change.
    last_value: Option<T>,
    change_state: ChangeState,
}

impl<T> NetworkVar<T> {
    /// Has the value changed since the given tick?
    pub fn has_changed_since(&self, tick: u32) -> bool {
        match self.change_state {
            ChangeState::Dirty => true,
            ChangeState::Clean { last_changed_tick } => last_changed_tick > tick,
        }
    }
}

impl<T> From<T> for NetworkVar<T> {
    fn from(value: T) -> Self {
        Self {
            value,
            last_value: None,
            change_state: ChangeState::Dirty,
        }
    }
}

impl<T> Deref for NetworkVar<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> DerefMut for NetworkVar<T>
where
    T: Clone,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.last_value = Some(self.value.clone());
        &mut self.value
    }
}

#[derive(Default)]
enum ChangeState {
    /// Changed this tick.
    #[default]
    Dirty,
    /// Changed in the past.
    Clean { last_changed_tick: u32 },
}

/// A variable that is received from the server by the client. Counterpart to [`NetworkVar`].
pub struct ServerVar<T> {
    // This is an Option because the component is inserted into the world before we can set the value from the server.
    // We ignore this in the public api, as no code should be able to access it by accident between creation and initialization.
    // In short: oh god this is terrible.
    value: Option<T>,
}

impl<T> ServerVar<T> {
    pub fn set(&mut self, value: T) {
        self.value = Some(value);
    }

    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }
}

impl<T> Default for ServerVar<T> {
    fn default() -> Self {
        Self {
            value: Default::default(),
        }
    }
}

const UNINITIALIZED_ACCESS_ERROR: &str = "Server variable was accessed before being initialized";

impl<T> Deref for ServerVar<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref().expect(UNINITIALIZED_ACCESS_ERROR)
    }
}

impl<T> DerefMut for ServerVar<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut().expect(UNINITIALIZED_ACCESS_ERROR)
    }
}

/// A message that contains data for a component.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct NetworkedComponentMessage {
    identity: NetworkIdentity,
    component_id: ComponentNetworkId,
    data: Bytes,
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct ComponentNetworkId(u16);

impl ComponentNetworkId {
    const fn max() -> usize {
        u16::MAX as usize
    }
}

/// Maps component uuids to a smaller data type to save network bandwith.
#[derive(Default)]
struct NetworkedComponentRegistry {
    components: Vec<Uuid>,
}

impl NetworkedComponentRegistry {
    fn register<T: NetworkedFromServer>(&mut self) -> bool {
        if self.components.len() >= ComponentNetworkId::max() {
            panic!("Too many different network components registered.");
        }

        let uuid = T::TYPE_UUID;
        // Components must be sorted by UUID so the index is always the same
        if let Err(pos) = self.components.binary_search(&uuid) {
            self.components.insert(pos, uuid);
            return true;
        }
        false
    }

    fn get_id(&self, uuid: &Uuid) -> Option<ComponentNetworkId> {
        self.components
            .binary_search(uuid)
            .ok()
            .map(|i| ComponentNetworkId(i as u16))
    }

    fn get_uuid(&self, id: ComponentNetworkId) -> Option<&Uuid> {
        self.components.get(id.0 as usize)
    }
}

#[derive(Serialize, Deserialize)]
pub struct ValueUpdate<'a, T: Clone>(pub Cow<'a, T>);

impl<'a, T: Clone> From<&'a T> for ValueUpdate<'a, T> {
    fn from(v: &'a T) -> Self {
        ValueUpdate(Cow::Borrowed(v))
    }
}

impl<T: std::clone::Clone> From<T> for ValueUpdate<'static, T> {
    fn from(v: T) -> Self {
        ValueUpdate(Cow::Owned(v))
    }
}

trait Diffable {
    type Diff;

    fn diff(&self, from: &Self) -> Self::Diff;
    fn apply(&mut self, diff: &Self::Diff);
}

// TODO: Actually implement this lmao nice try
#[derive(Serialize, Deserialize)]
enum DiffableValueUpdate<'a, T: Clone + Diffable> {
    Full(Cow<'a, T>),
    Delta { from_tick: u32, diff: T::Diff },
}

fn send_networked_component_to_new<S: NetworkedToClient, C: NetworkedFromServer>(
    mut components: Query<(&NetworkIdentity, &mut S)>,
    visibilities: Res<NetworkVisibilities>,
    registry: Res<NetworkedComponentRegistry>,
    mut sender: MessageSender,
    param: bevy::ecs::system::StaticSystemParam<S::Param>,
) {
    for (identity, mut component) in components.iter_mut() {
        let visibility = match visibilities.visibility.get(identity) {
            Some(v) => v,
            None => continue,
        };

        let component_id = registry
            .get_id(&C::TYPE_UUID)
            .expect("Networked component incorrectly registered");
        if S::receiver_matters() {
            // Serialize component for every receiver
            for connection in visibility.new_observers() {
                let data = match component.serialize(&*param, Some(*connection), None) {
                    Some(d) => d,
                    None => continue,
                };

                sender.send(
                    &NetworkedComponentMessage {
                        identity: *identity,
                        component_id,
                        data,
                    },
                    MessageReceivers::Single(*connection),
                );
            }
        } else {
            let new_observers: HashSet<_> = visibility.new_observers().copied().collect();
            if !new_observers.is_empty() {
                let data = component
                    .serialize(&*param, None, None)
                    .expect("Serializing without a specific receiver should always return data");
                sender.send(
                    &NetworkedComponentMessage {
                        identity: *identity,
                        component_id,
                        data,
                    },
                    MessageReceivers::Set(new_observers),
                );
            }
        }
    }
}

/// Buffers initial networked component values for scenes that aren't spawned yet
struct BufferedNetworkedComponents<C> {
    to_apply: Vec<(InstanceId, NetworkedComponentMessage, Entity)>,
    phantom_data: PhantomData<C>,
}

impl<C> Default for BufferedNetworkedComponents<C> {
    fn default() -> Self {
        Self {
            to_apply: Default::default(),
            phantom_data: Default::default(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn receive_networked_component<C: NetworkedFromServer>(
    mut events: EventReader<MessageEvent<NetworkedComponentMessage>>,
    mut components: Query<&mut C>,
    scene_instances: Query<&SceneInstance>,
    registry: Res<NetworkedComponentRegistry>,
    identities: Res<NetworkIdentities>,
    mut buffer: ResMut<BufferedNetworkedComponents<C>>,
    param: bevy::ecs::system::StaticSystemParam<C::Param>,
    mut commands: Commands,
) {
    for event in events.iter() {
        // TODO: Move the id->uuid conversion into one system for performance?
        // Check if the message is for this component
        let uuid = registry
            .get_uuid(event.message.component_id)
            .expect("Received component message for unknown component");
        if uuid != &C::TYPE_UUID {
            continue;
        }

        let target = event.message.identity;
        let entity = match identities.get_entity(target) {
            Some(e) => e,
            None => {
                warn!(
                    identity = ?target,
                    "Received component message for non-existent identity"
                );
                continue;
            }
        };

        if let Ok(scene_instance) = scene_instances.get(entity) {
            buffer
                .to_apply
                .push((**scene_instance, event.message.clone(), entity));
            continue;
        }

        apply_component_update(
            entity,
            &event.message,
            &mut components,
            &param,
            &mut commands,
        );
    }
}

// TODO: DRY up a bit
fn apply_networked_component_to_scene<C: NetworkedFromServer>(
    mut buffer: ResMut<BufferedNetworkedComponents<C>>,
    mut components: Query<&mut C>,
    scene_spawner: Res<SceneSpawner>,
    child_query: Query<&Children>,
    param: bevy::ecs::system::StaticSystemParam<C::Param>,
    mut commands: Commands,
) where
    <C as NetworkedFromServer>::Param: SystemParam + 'static,
{
    buffer.to_apply.retain(|(instance, message, entity)| {
        // Keep in buffer if scene not spawned
        if !scene_spawner.instance_is_ready(*instance) {
            return true;
        }

        let children = child_query
            .get(*entity)
            .expect("Parent of spawned scene should have children");
        // Try to get scene root
        let root = *match children.deref() {
            [c] => c,
            [] => {
                panic!("Scene parent should have at least one child")
            }
            [..] => {
                warn!(
                    ?entity,
                    "Networked components are only supported on scenes with one root entity"
                );
                return false;
            }
        };

        apply_component_update(root, message, &mut components, &param, &mut commands);

        false
    });
}

fn apply_component_update<C: NetworkedFromServer>(
    entity: Entity,
    message: &NetworkedComponentMessage,
    components: &mut Query<&mut C>,
    param: &bevy::ecs::system::StaticSystemParam<C::Param>,
    commands: &mut Commands,
) {
    match components.get_mut(entity) {
        Ok(mut c) => c.deserialize(&*param, &message.data),
        Err(_) => {
            // Apply data to default component value if possible
            if let Some(mut default) = C::default_if_missing() {
                default.deserialize(&*param, &message.data);
                commands.entity(entity).insert(*default);
            } else {
                warn!(
                    ?entity,
                    component = std::any::type_name::<C>(),
                    "Received component message for entity without that component"
                );
            }
        }
    };
}

pub trait AppExt {
    fn add_networked_component<S, C>(&mut self) -> &mut App
    where
        S: NetworkedToClient,
        C: NetworkedFromServer;
}

impl AppExt for App {
    /// Registers a networked component.
    /// Changes are synced from the server component (`S`) to the client component (`C`).
    fn add_networked_component<S, C>(&mut self) -> &mut App
    where
        S: NetworkedToClient,
        C: NetworkedFromServer,
    {
        self.init_resource::<NetworkedComponentRegistry>();
        let mut registry = self.world.resource_mut::<NetworkedComponentRegistry>();
        if !registry.register::<C>() {
            panic!("Client component was already registered");
        }
        if self.world.resource::<NetworkManager>().is_server() {
            self.add_system(
                send_networked_component_to_new::<S, C>
                    .before(MessagingSystem::SendOutbound)
                    .after(NetworkSystem::Visibility)
                    .after(SpawningSystems::Spawn),
            );
        } else {
            self.init_resource::<BufferedNetworkedComponents<C>>()
                .add_system(
                    receive_networked_component::<C>
                        .before(NetworkSystem::ReadNetworkMessages)
                        .label(ComponentSystem::Apply),
                )
                .add_system_to_stage(
                    PostSceneSpawnerStage,
                    apply_networked_component_to_scene::<C>.label(ComponentSystem::Apply),
                );
        }
        self
    }
}

/// A stage that runs after the scene spawner and before any other stages.
/// This is required so we can apply network variables before the components are accessed, giving the illusion of them always being there.
// TODO: Replace this with a scene spawning modification?
#[derive(Debug, Hash, PartialEq, Eq, Clone, StageLabel)]
pub struct PostSceneSpawnerStage;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
pub enum ComponentSystem {
    Apply,
}

pub(crate) struct ComponentPlugin;

impl Plugin for ComponentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedComponentRegistry>()
            .add_network_message::<NetworkedComponentMessage>();

        if app.world.resource::<NetworkManager>().is_client() {
            // Only need to apply networked components on client, as they only go that direction
            app.add_stage_after(
                CoreStage::PreUpdate,
                PostSceneSpawnerStage,
                SystemStage::parallel(),
            );
        }
    }
}

/* #[derive(Component)]
struct ExampleComponent {
    health: NetworkVar<f32>,
    inventory: NetworkVar<Vec<NetworkIdentity>>,
    other_var: bool,
}

impl NetworkedToClient for ExampleComponent {
    type Param = ();

    fn receiver_matters() -> bool {
        false
    }

    fn serialize(
        &mut self,
        _: Self::Param,
        _: Option<ConnectionId>,
        since_tick: Option<NonZeroU32>,
    ) -> Option<Bytes> {
        // TODO: Reserve smart amount
        let mut writer = BytesMut::with_capacity(2).writer();
        let mut serializer = bincode::Serializer::new(&mut writer, bincode::options());

        let health_changed = since_tick
            .map(|t| self.health.has_changed_since(t.into()))
            .unwrap_or(true);
        health_changed
            .then(|| ValueUpdate::from(&self.health.value))
            .serialize(&mut serializer)
            .unwrap();

        let inventory_changed = since_tick
            .map(|t| self.inventory.has_changed_since(t.into()))
            .unwrap_or(true);
        // TODO: Diff
        inventory_changed
            .then(|| ValueUpdate::from(&self.inventory.value))
            .serialize(&mut serializer)
            .unwrap();

        Some(writer.into_inner().into())
    }
}

#[derive(Component, TypeUuid)]
#[uuid = "02de843e-5491-4989-9991-60055d333a4b"]
struct ExampleComponentClient {
    health: ServerVar<f32>,
}

impl NetworkedFromServer for ExampleComponentClient {
    type Param = ();
    fn deserialize(&mut self, _: Self::Param, data: &[u8]) {
        let mut deserializer =
            bincode::Deserializer::with_reader(data.reader(), bincode::options());
        let health_update = Option::<ValueUpdate<f32>>::deserialize(&mut deserializer)
            .expect("Error deserializing networked component");
        if let Some(health_update) = health_update {
            self.health.set(health_update.0.into_owned());
        }
        // TODO: Debug assert that we've consumed all data
    }
}
 */
