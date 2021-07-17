use bevy::{
    math::UVec2,
    prelude::{Commands, Entity, Query, RemovedComponents},
    utils::HashMap,
};

use super::Item;

pub struct Container {
    size: UVec2,
    items: HashMap<UVec2, Entity>,
}

impl Container {
    pub fn new(size: UVec2) -> Self {
        Self {
            size,
            items: Default::default(),
        }
    }

    pub fn insert_item_unchecked(&mut self, entity: Entity, position: UVec2) {
        self.items.insert(position, entity);
    }

    pub fn remove_item(&mut self, entity: Entity) {
        let entry = self.items.iter().filter(|(_, v)| v == &&entity).next();
        if let Some((&k, _)) = entry {
            self.items.remove(&k);
        }
    }
}

pub type ContainerQuery<'a> = Query<'a, (&'static Item,)>;

pub struct ContainerItemIterator<'a, 'b: 'a> {
    query: &'a ContainerQuery<'b>,
    inner_iter: std::collections::hash_map::Iter<'a, UVec2, Entity>,
}

impl<'a, 'b> Iterator for ContainerItemIterator<'a, 'b> {
    type Item = (&'a UVec2, &'a Item);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = self.inner_iter.next()?;
            if let Ok((item,)) = self.query.get(entry.1.clone()) {
                return Some((entry.0, item));
            }
        }
    }
}

impl<'a, 'b> From<&ContainerAccessor<'a, 'b>> for ContainerItemIterator<'a, 'b> {
    fn from(accessor: &ContainerAccessor<'a, 'b>) -> Self {
        Self {
            query: accessor.query,
            inner_iter: accessor.container.items.iter(),
        }
    }
}

pub struct ContainerAccessor<'a, 'b: 'a> {
    container: &'a Container,
    query: &'a ContainerQuery<'b>,
}

impl<'a, 'b> ContainerAccessor<'a, 'b> {
    pub fn new(container: &'a Container, query: &'a ContainerQuery<'b>) -> Self {
        Self { container, query }
    }

    pub fn can_fit(&self, item: &Item, position: UVec2) -> bool {
        for (&other_position, &entity) in self.container.items.iter() {
            let other_item = self.query.get(entity).unwrap();

            let x_overlap = (position.x as i32 - other_position.x as i32).abs() as u32 * 2
                < item.size.x + other_item.0.size.x;
            let y_overlap = (position.y as i32 - other_position.y as i32).abs() as u32 * 2
                < item.size.y + other_item.0.size.y;

            if x_overlap && y_overlap {
                return false;
            }
        }

        true
    }

    pub fn items(&self) -> ContainerItemIterator {
        self.into()
    }
}

pub struct ContainerWriter<'a> {
    container: &'a mut Container,
    entity: Entity,
    query: &'a ContainerQuery<'a>,
}

impl<'a> ContainerWriter<'a> {
    pub fn new(
        container: &'a mut Container,
        entity: Entity,
        query: &'a ContainerQuery<'a>,
    ) -> Self {
        Self {
            container,
            query,
            entity,
        }
    }

    pub fn insert_item<'b>(&'a mut self, item: &mut Item, item_entity: Entity, position: UVec2) {
        if !ContainerAccessor::new(self.container, self.query.clone()).can_fit(item, position) {
            return;
        }

        self.container.insert_item_unchecked(item_entity, position);
        item.container = Some(self.entity);
    }
}

pub fn cleanup_removed_items_system(
    removed_items: RemovedComponents<Item>,
    mut containers: Query<(&mut Container,)>,
) {
    for entity in removed_items.iter() {
        for (mut container,) in containers.iter_mut() {
            let mut key = None;
            if let Some((&k, _)) = container.items.iter().find(|(_, ent)| **ent == entity) {
                key = Some(k);
            }
            if let Some(k) = key {
                container.items.remove(&k);
                continue;
            }
        }
    }
}
