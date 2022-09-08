use bevy::{
    math::{IVec2, Vec2, Vec3Swizzles},
    prelude::{
        Added, App, Changed, Component, CoreStage, Entity, GlobalTransform, Or,
        ParallelSystemDescriptorCoercion, Plugin, Query, Res, ResMut, SystemLabel, UVec2,
    },
    transform::TransformSystem,
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

    /// Returns all observers. Includes observers that were just removed.
    pub(crate) fn all_observers(&self) -> impl Iterator<Item = &ConnectionId> {
        self.observers.iter().map(|(c, _)| c)
    }
}

/// Stores a mapping between network identities and their observers
#[derive(Default)]
pub(crate) struct NetworkVisibilities {
    pub(crate) visibility: HashMap<NetworkIdentity, NetworkVisibility>,
}

#[derive(Default, Debug)]
struct SpatialCell {
    entities: HashSet<Entity>,
}

#[derive(Debug)]
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

    fn insert(&mut self, entity: Entity, position: IVec2, aabb: GridAabb, current: &mut InGrid) {
        let changed_position = current.position != Some(position);
        let changed_aabb = current.aabb != aabb;

        if !changed_position && !changed_aabb {
            return;
        }

        // Remove from old cell(s)
        if let Some(old) = current.position {
            for position in self.relevant_positions(old + current.aabb.center, current.aabb.size) {
                if let Some(cell) = self.cells.get_mut(&position) {
                    cell.entities.remove(&entity);
                };
            }
        }

        if changed_position {
            current.position = Some(position);
        }
        if changed_aabb {
            current.aabb = aabb;
        }

        // Insert into new cell(s)
        for position in self.relevant_positions(position + aabb.center, aabb.size) {
            let cell = self.cells.entry(position).or_default();
            cell.entities.insert(entity);
        }
    }

    fn relevant_cells(&self, position: IVec2, size: UVec2) -> impl Iterator<Item = &SpatialCell> {
        self.relevant_positions(position, size)
            .flat_map(|pos| self.cells.get(&pos))
    }

    fn relevant_positions(&self, position: IVec2, size: UVec2) -> impl Iterator<Item = IVec2> {
        ((position.x - size.x as i32)..=(position.x + size.x as i32)).flat_map(move |x| {
            ((position.y - size.y as i32)..=(position.y + size.y as i32))
                .map(move |y| IVec2::new(x, y))
        })
    }
}

type GlobalGrid = SpatialHash;
/// The size of a side of a quadratic cell in the global grid
pub const GLOBAL_GRID_CELL_SIZE: u16 = 10;

#[derive(Component, Default)]
pub(crate) struct InGrid {
    /// Where this entity is in the global grid
    position: Option<IVec2>,
    aabb: GridAabb,
}

/// A component that sets the size and center of the object in the visibility grid.
/// This is only required for objects that are massive (bigger than a chunk).
#[derive(Component, Default, PartialEq, Eq, Clone, Copy)]
pub struct GridAabb {
    /// The size in the grid in half-extents
    pub size: UVec2,
    /// How to offset the entity position in the grid
    pub center: IVec2,
}

fn global_grid_update(
    mut grid: ResMut<GlobalGrid>,
    mut query: Query<
        (Entity, &GlobalTransform, &mut InGrid, Option<&GridAabb>),
        Or<(Changed<GlobalTransform>, Added<InGrid>, Changed<GridAabb>)>,
    >,
) {
    for (entity, transform, mut in_grid, grid_aabb) in query.iter_mut() {
        let new_cell = grid.cell_position(transform.translation().xz());

        grid.insert(
            entity,
            new_cell,
            grid_aabb.copied().unwrap_or_default(),
            &mut in_grid,
        );
        in_grid.position = Some(new_cell);
    }
}

// TODO: Remove deleted entities from grid
fn grid_visibility(
    mut visibilities: ResMut<NetworkVisibilities>,
    grid: Res<GlobalGrid>,
    observers: Query<(&NetworkObserver, &InGrid)>,
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

        for cell in grid.relevant_cells(position, UVec2::new(observer.range, observer.range)) {
            bevy::log::info!(position = ?UVec2::new(observer.range, observer.range), count=cell.entities.len(), "AAA");
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
pub enum VisibilitySystem {
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
                .insert_resource(GlobalGrid {
                    cell_size: GLOBAL_GRID_CELL_SIZE,
                    ..Default::default()
                })
                .add_system_to_stage(
                    CoreStage::PostUpdate,
                    global_grid_update
                        .label(VisibilitySystem::UpdateGrid)
                        .after(TransformSystem::TransformPropagate),
                )
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
