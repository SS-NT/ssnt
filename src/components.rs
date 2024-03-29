use bevy::{
    ecs::system::{Command, EntityCommands},
    prelude::*,
};

#[derive(Component)]
pub struct Disabled<T>(pub T);

pub struct EnableComponent<T> {
    pub entity: Entity,
    pub phantom: std::marker::PhantomData<T>,
}

impl<T> EnableComponent<T> {
    pub fn new(entity: Entity) -> Self {
        Self {
            entity,
            phantom: Default::default(),
        }
    }
}

impl<T> Command for EnableComponent<T>
where
    T: Component,
{
    fn apply(self, world: &mut World) {
        let mut entity = world.entity_mut(self.entity);
        let value = entity.take::<Disabled<T>>().unwrap().0;
        entity.insert(value);
    }
}

pub struct DisableComponent<T> {
    pub entity: Entity,
    pub phantom: std::marker::PhantomData<T>,
}

impl<T> DisableComponent<T> {
    pub fn new(entity: Entity) -> Self {
        Self {
            entity,
            phantom: Default::default(),
        }
    }
}

impl<T> Command for DisableComponent<T>
where
    T: Component,
{
    fn apply(self, world: &mut World) {
        let mut entity = world.entity_mut(self.entity);
        let value = entity.take::<T>().unwrap();
        entity.insert(Disabled(value));
    }
}

pub trait EntityCommandsExt {
    fn enable_component<T>(&mut self) -> &mut Self
    where
        T: Component;
    fn disable_component<T>(&mut self) -> &mut Self
    where
        T: Component;
}

impl<'w, 's, 'a> EntityCommandsExt for EntityCommands<'w, 's, 'a> {
    fn enable_component<T>(&mut self) -> &mut Self
    where
        T: Component,
    {
        let id = self.id();
        self.commands().add(EnableComponent::<T>::new(id));
        self
    }

    fn disable_component<T>(&mut self) -> &mut Self
    where
        T: Component,
    {
        let id = self.id();
        self.commands().add(DisableComponent::<T>::new(id));
        self
    }
}

/// Despawns all entities that have a specific component
pub fn despawn_with<T: Component>(to_despawn: Query<Entity, With<T>>, mut commands: Commands) {
    for entity in &to_despawn {
        commands.entity(entity).despawn_recursive();
    }
}
