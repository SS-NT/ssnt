use std::{hash::Hash, marker::PhantomData, num::NonZeroU32};

use bevy::{
    ecs::{event::Event, system::SystemParam},
    prelude::{App, EventReader, EventWriter, ResMut, Resource},
};

#[derive(Event)]
pub struct Order<T: 'static> {
    id: OrderId<T>,
    data: T,
}

impl<T> Order<T> {
    #[must_use]
    pub fn id(&self) -> OrderId<T> {
        self.id
    }

    #[must_use]
    pub fn data(&self) -> &T {
        &self.data
    }

    #[must_use]
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }

    /// Returns an order result for this order. It must be sent as an event to actually signal completion.
    #[must_use]
    pub fn complete<R>(&self, result: R) -> OrderResult<T, R> {
        OrderResult::new(self.id, result)
    }
}

pub struct OrderId<T: 'static> {
    id: NonZeroU32,
    phantom: PhantomData<fn() -> T>,
}

impl<T: 'static> Hash for OrderId<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T> Copy for OrderId<T> {}
impl<T> Clone for OrderId<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Eq for OrderId<T> {}
impl<T> PartialEq for OrderId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[derive(Resource)]
#[doc(hidden)]
pub struct InternalOrderRes<T> {
    next_id: NonZeroU32,
    phantom: PhantomData<fn() -> T>,
}

impl<T> Default for InternalOrderRes<T> {
    fn default() -> Self {
        Self {
            next_id: NonZeroU32::new(1).unwrap(),
            phantom: Default::default(),
        }
    }
}

impl<T> InternalOrderRes<T> {
    fn next_id(&mut self) -> OrderId<T> {
        let id = OrderId {
            id: self.next_id,
            phantom: Default::default(),
        };
        self.next_id = self.next_id.checked_add(1).unwrap();
        id
    }
}

#[derive(Event)]
pub struct OrderResult<O: 'static, R> {
    pub id: OrderId<O>,
    pub data: R,
}

impl<O: 'static, R> OrderResult<O, R> {
    pub fn new(id: OrderId<O>, data: R) -> Self {
        Self { id, data }
    }
}

pub trait OrderAppExt {
    fn register_order<O, R>(&mut self) -> &mut App
    where
        O: Event,
        R: Event;
}

impl OrderAppExt for App {
    fn register_order<O, R>(&mut self) -> &mut App
    where
        O: Event,
        R: Event,
    {
        self.init_resource::<InternalOrderRes<O>>()
            .add_event::<Order<O>>()
            .add_event::<OrderResult<O, R>>()
    }
}

/// System param used to create orders that will be completed by another system.
#[derive(SystemParam)]
pub struct Orderer<'w, T: Event> {
    writer: EventWriter<'w, Order<T>>,
    res: ResMut<'w, InternalOrderRes<T>>,
}

impl<'w, T: Event> Orderer<'w, T> {
    /// Creates an order and returns the id used to retrieve the result.
    pub fn create(&mut self, order: T) -> OrderId<T> {
        let id = self.res.next_id();
        self.writer.send(Order { id, data: order });
        id
    }
}

#[derive(SystemParam)]
pub struct Results<'w, 's, O: 'static, R: Event> {
    reader: EventReader<'w, 's, OrderResult<O, R>>,
}

impl<'w, 's, O: 'static, R: Event> Results<'w, 's, O, R> {
    /// Gets the result of an order.
    ///
    /// Note: do not use this if you have multiple pending orders, as this method may skip them.
    pub fn get(&mut self, id: OrderId<O>) -> Option<&R> {
        self.reader.iter().find(|e| e.id == id).map(|e| &e.data)
    }

    pub fn iter(
        &mut self,
    ) -> impl Iterator<Item = &OrderResult<O, R>> + ExactSizeIterator<Item = &OrderResult<O, R>>
    {
        self.reader.iter()
    }
}
