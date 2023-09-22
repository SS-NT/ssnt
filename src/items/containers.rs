use bevy::{prelude::*, utils::HashMap};
use networking::{
    identity::NetworkIdentity,
    is_server,
    spawning::ClientControls,
    visibility::{NetworkVisibilities, VisibilitySystem},
    NetworkSet, Players,
};
use physics::PhysicsEntityCommands;
use utils::task::{Task, Tasks};

use super::{Item, StoredItem};

pub struct ContainerPlugin;

impl Plugin for ContainerPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Container>()
            .register_type::<DisplayContainer>();
        if is_server(app) {
            app.init_resource::<Tasks<MoveItem>>()
                .add_systems(
                    PreUpdate,
                    item_in_container_visibility
                        .in_set(NetworkSet::ServerVisibility)
                        .after(VisibilitySystem::GridVisibility),
                )
                .add_systems(Update, do_item_move);
        }
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Container {
    size: UVec2,
    items: HashMap<UVec2, Entity>,
    /// What entity any items should be nested under. Defaults to the container entity if `None`.
    pub attach_to: Option<Entity>,
    pub relative_position: Vec3,
    /// The items remain visible when stored
    pub items_visible: bool,
}

impl FromWorld for Container {
    fn from_world(_: &mut World) -> Self {
        Container {
            size: (1, 1).into(),
            items: Default::default(),
            attach_to: None,
            relative_position: Vec3::ZERO,
            items_visible: false,
        }
    }
}

impl Container {
    pub fn insert_item_unchecked(&mut self, entity: Entity, position: UVec2) {
        self.items.insert(position, entity);
    }

    pub fn remove_item(&mut self, entity: Entity) {
        let entry = self.items.iter().find(|(_, v)| v == &&entity);
        if let Some((&k, _)) = entry {
            self.items.remove(&k);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn can_fit(&self, items_query: &Query<&Item>, item: &Item, position: UVec2) -> bool {
        if position.x + item.size.x > self.size.x || position.y + item.size.y > self.size.y {
            return false;
        }

        for (&other_position, &entity) in self.items.iter() {
            let other_item = items_query.get(entity).unwrap();

            let x_overlap = (position.x as i32 - other_position.x as i32).unsigned_abs() * 2
                < item.size.x + other_item.size.x;
            let y_overlap = (position.y as i32 - other_position.y as i32).unsigned_abs() * 2
                < item.size.y + other_item.size.y;

            if x_overlap && y_overlap {
                return false;
            }
        }

        true
    }

    pub fn iter(&self) -> impl Iterator<Item = (&UVec2, &Entity)> {
        self.items.iter()
    }
}

/// A component on containers which show their contents to everyone in the area.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct DisplayContainer;

/// An event requesting to move an item from or into a container.
#[derive(Debug)]
pub struct MoveItem {
    pub item: Entity,
    pub container: Option<Entity>,
    pub position: Option<UVec2>,
}

impl Task for MoveItem {
    type Result = MoveItemResult;
}

pub struct MoveItemResult {
    success: bool,
}

impl MoveItemResult {
    pub fn was_success(&self) -> bool {
        self.success
    }
}

fn do_item_move(
    mut tasks: ResMut<Tasks<MoveItem>>,
    mut containers: Query<&mut Container>,
    mut items: Query<(Entity, &Item, Option<&mut StoredItem>)>,
    global_transforms: Query<&GlobalTransform>,
    only_items: Query<&Item>,
    mut commands: Commands,
) {
    tasks.process(|data| {
        let Ok((item_entity, item, mut stored)) = items.get_mut(data.item) else {
            warn!(task = ?data, "Failed to move item because it does not have an item component");
            return MoveItemResult { success: false };
        };

        // Remove from old container if it exists
        if let Some(stored) = stored.as_mut() {
            let mut container = containers.get_mut(*stored.container).unwrap();
            container.remove_item(item_entity);
        }

        let Some(container_entity) = data.container else {
            // If we're putting it back into the world
            if stored.is_some() {
                let mut entity_commands = commands.entity(item_entity);
                entity_commands
                    .remove::<StoredItem>()
                    .remove_parent()
                    .enable_physics();

                if let Ok(transform) = global_transforms.get(item_entity) {
                    entity_commands.insert(Transform::from(*transform));
                }
            }
            return MoveItemResult { success: true };
        };

        let Ok(mut container) = containers.get_mut(container_entity) else {
            warn!(task = ?data, "Failed to move item because target is not a container");
            return MoveItemResult { success: false };
        };

        let position = data.position.expect("Automatic position not supported yet");
        if !container.can_fit(&only_items, item, position) {
            warn!(task = ?data, "Failed to move item because it does not fit in the container");
            return MoveItemResult { success: false };
        }

        container.insert_item_unchecked(data.item, position);
        if let Some(stored) = stored.as_mut() {
            *stored.container = container_entity;
            *stored.visible = container.items_visible;
        } else {
            commands.entity(data.item).insert(StoredItem {
                container: container_entity.into(),
                visible: container.items_visible.into(),
            });
        }

        // TODO: Do all containers nest their items? Probably...
        commands
            .entity(container.attach_to.unwrap_or(container_entity))
            .add_child(data.item);
        // Freeze the item as a child
        commands
            .entity(item_entity)
            .insert(Transform::default())
            .disable_physics();

        MoveItemResult { success: true }
    });
}

/// Influences the network visibility of items that are stored in a container.
fn item_in_container_visibility(
    items: Query<(&StoredItem, &NetworkIdentity)>,
    containers: Query<(Entity, Option<&DisplayContainer>), With<Container>>,
    mut visibilities: ResMut<NetworkVisibilities>,
    parents: Query<&Parent>,
    controls: Res<ClientControls>,
    players: Res<Players>,
) {
    for (item, identity) in items.iter() {
        let Some(visibility) = visibilities.get_mut(*identity) else {
            continue;
        };

        let (container_entity, display) = containers.get(*item.container).unwrap();

        if display.is_none() {
            // Remove all observers by default for non-displaying containers
            visibility.remove_observers();

            // Add any parent player as an observer (players should see all the containers on their character)
            // TODO: Is this actually the best idea?
            if let Some(connection) = parents
                .iter_ancestors(container_entity)
                .find_map(|root| controls.controlling_player(root))
                .and_then(|player| players.get_connection(&player))
            {
                visibility.add_observer(connection);
            }

            // TODO: Handle players looking into the container (having UI open)
        }
    }
}
