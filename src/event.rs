use bevy::{
    ecs::schedule::{SystemDescriptor, SystemLabelId},
    prelude::*,
};

pub trait InterceptableEvent: Send + Sync + 'static {
    fn start_label() -> SystemLabelId;
    fn end_label() -> SystemLabelId;
}

#[derive(Resource)]
pub struct InterceptableEvents<T: InterceptableEvent> {
    events: Vec<T>,
}

impl<T: InterceptableEvent> Default for InterceptableEvents<T> {
    fn default() -> Self {
        Self {
            events: Default::default(),
        }
    }
}

impl<T: InterceptableEvent> InterceptableEvents<T> {
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.events.iter()
    }

    pub fn drain<R: std::ops::RangeBounds<usize>>(&mut self, range: R) -> std::vec::Drain<'_, T> {
        self.events.drain(range)
    }

    pub fn push(&mut self, value: T) {
        self.events.push(value)
    }
}

pub trait EventAppExt {
    fn add_interceptable_event<T: InterceptableEvent>(&mut self) -> &mut App;
}

impl EventAppExt for App {
    fn add_interceptable_event<T: InterceptableEvent>(&mut self) -> &mut App {
        self.insert_resource(InterceptableEvents::<T>::default())
    }
}

pub trait EventSystemExt<Params> {
    fn intercept<T: InterceptableEvent>(self) -> SystemDescriptor;
}

impl EventSystemExt<()> for SystemDescriptor {
    fn intercept<T: InterceptableEvent>(self) -> SystemDescriptor {
        self.after(T::start_label()).before(T::end_label())
    }
}
