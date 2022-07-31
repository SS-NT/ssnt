use bevy::{
    math::{IVec2, Vec2, Vec3Swizzles},
    prelude::{
        App, Changed, Component, Entity, GlobalTransform, ParallelSystemDescriptorCoercion, Plugin,
        Query, Res, ResMut, SystemLabel,
    },
    utils::{hashbrown::hash_map::Entry, HashMap, HashSet},
};

use crate::{identity::NetworkIdentity, ConnectionId, NetworkManager, NetworkSystem};

/// Allows connections to observer networked objects in range
#[derive(Component)]
pub struct NetworkObserver {
    pub range: u32,
    pub connection: ConnectionId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObserverState {
    /// Observation started this frame
    Added,
    /// Has observed for at least one frame
    Observing,
    /// Observation stopped this frame
    Removed,
}

/// Stores which connections are observing something
#[derive(Default)]
pub struct NetworkVisibility {
    observers: HashMap<ConnectionId, ObserverState>,
}

impl NetworkVisibility {
    fn add_observer(&mut self, connection: ConnectionId) {
        self.observers
            .entry(connection)
            .and_modify(|s| {
                if let ObserverState::Removed = s {
                    *s = ObserverState::Observing;
                }
            })
            .or_insert(ObserverState::Added);
    }

    fn remove_observer(&mut self, connection: ConnectionId) {
        if let Entry::Occupied(mut o) = self.observers.entry(connection) {
            match o.get() {
                ObserverState::Added => {
                    o.remove_entry();
                }
                ObserverState::Observing => {
                    *o.get_mut() = ObserverState::Removed;
                }
                ObserverState::Removed => {}
            }
        };
    }

    fn update(&mut self) {
        self.observers.retain(|_, state| match state {
            ObserverState::Added => {
                *state = ObserverState::Observing;
                true
            }
            ObserverState::Observing => true,
            ObserverState::Removed => false,
        });
    }

    pub fn observers(&self) -> impl Iterator<Item = &ConnectionId> {
        self.observers.iter().filter_map(|(id, s)| match s {
            ObserverState::Added | ObserverState::Observing => Some(id),
            ObserverState::Removed => None,
        })
    }

    pub fn new_observers(&self) -> impl Iterator<Item = &ConnectionId> {
        self.observers.iter().filter_map(|(id, s)| match s {
            ObserverState::Added => Some(id),
            _ => None,
        })
    }

    pub fn removed_observers(&self) -> impl Iterator<Item = &ConnectionId> {
        self.observers.iter().filter_map(|(id, s)| match s {
            ObserverState::Removed => Some(id),
            _ => None,
        })
    }

    pub fn has_observer(&self, connection: &ConnectionId) -> bool {
        self.observers
            .get(connection)
            .map(|s| *s != ObserverState::Removed)
            == Some(true)
    }
}

/// Stores a mapping between network identities and their observers
#[derive(Default)]
pub(crate) struct NetworkVisibilities {
    pub(crate) visibility: HashMap<NetworkIdentity, NetworkVisibility>,
}

#[derive(Default)]
struct SpatialCell {
    entities: HashSet<Entity>,
}

struct SpatialHash {
    cells: HashMap<IVec2, SpatialCell>,
    cell_size: u16,
}

impl Default for SpatialHash {
    fn default() -> Self {
        Self {
            cells: Default::default(),
            cell_size: 10,
        }
    }
}

impl SpatialHash {
    fn cell_position(&self, position: Vec2) -> IVec2 {
        position.as_ivec2() / IVec2::new(self.cell_size.into(), self.cell_size.into())
    }

    fn insert(&mut self, entity: Entity, position: IVec2, old_position: Option<IVec2>) {
        // Remove from old cell
        if let Some(old) = old_position {
            if let Some(cell) = self.cells.get_mut(&old) {
                cell.entities.remove(&entity);
            }
        }

        // Insert into new cell
        self.cells
            .entry(position)
            .or_default()
            .entities
            .insert(entity);
    }

    fn relevant_cells(&self, position: IVec2, range: u32) -> impl Iterator<Item = &SpatialCell> {
        ((position.x - range as i32)..=(position.x + range as i32)).flat_map(move |x| {
            ((position.y - range as i32)..=(position.y + range as i32))
                .flat_map(move |y| self.cells.get(&(x, y).into()))
        })
    }
}

type GlobalGrid = SpatialHash;

#[derive(Component, Default)]
pub(crate) struct GridPosition {
    /// Where this entity is in the global grid
    position: Option<IVec2>,
}

fn global_grid_update(
    mut grid: ResMut<GlobalGrid>,
    mut query: Query<(Entity, &GlobalTransform, &mut GridPosition), Changed<GlobalTransform>>,
) {
    for (entity, transform, mut grid_position) in query.iter_mut() {
        let new_cell = grid.cell_position(transform.translation().xz());
        if grid_position.position == Some(new_cell) {
            continue;
        }

        grid.insert(entity, new_cell, grid_position.position);
        grid_position.position = Some(new_cell);
    }
}

fn grid_visibility(
    mut visibilities: ResMut<NetworkVisibilities>,
    grid: Res<GlobalGrid>,
    observers: Query<(&NetworkObserver, &GridPosition)>,
    identities: Query<&NetworkIdentity>,
) {
    for (observer, grid_position) in observers.iter() {
        let position = match grid_position.position {
            Some(p) => p,
            None => continue,
        };

        let connection = observer.connection;

        for (_, vis) in visibilities.visibility.iter_mut() {
            vis.remove_observer(connection);
        }

        for cell in grid.relevant_cells(position, observer.range) {
            for entity in cell.entities.iter() {
                if let Ok(identity) = identities.get(*entity) {
                    let visibility = visibilities.visibility.entry(*identity).or_default();
                    visibility.add_observer(connection);
                }
            }
        }
    }
}

fn update_visibility(mut visibilities: ResMut<NetworkVisibilities>) {
    for (_, vis) in visibilities.visibility.iter_mut() {
        vis.update();
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
enum VisibilitySystem {
    UpdateGrid,
    UpdateVisibility,
}

pub(crate) struct VisibilityPlugin;

impl Plugin for VisibilityPlugin {
    fn build(&self, app: &mut App) {
        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.init_resource::<NetworkVisibilities>()
                .init_resource::<GlobalGrid>()
                .add_system(global_grid_update.label(VisibilitySystem::UpdateGrid))
                .add_system(update_visibility.label(VisibilitySystem::UpdateVisibility))
                .add_system(
                    grid_visibility
                        .label(NetworkSystem::Visibility)
                        .after(VisibilitySystem::UpdateGrid)
                        .after(VisibilitySystem::UpdateVisibility),
                );
        }
    }
}
