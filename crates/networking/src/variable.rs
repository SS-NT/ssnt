use std::{
    borrow::Cow,
    marker::PhantomData,
    num::NonZeroU32,
    ops::{Deref, DerefMut},
};

use bevy::{
    ecs::system::{SystemParam, SystemParamFetch},
    reflect::TypeUuid,
    utils::Uuid,
};
use serde::{Deserialize, Serialize};

use crate::ConnectionId;

pub use bytes::{Buf, BufMut, Bytes, BytesMut};
// TODO: Replace with handy method
pub use bincode::options as serializer_options;
pub use bincode::Deserializer as StandardDeserializer;
pub use bincode::Serializer as StandardSerializer;

/// A trait implemented by any component or resource that should be networked to clients.
pub trait NetworkedToClient {
    type Param: SystemParam;

    /// Does this serialize differently depending on who the receiver is?
    fn receiver_matters() -> bool;

    /// Serialize this to send it over the network.
    /// Returns `None` if it should not be sent to the given receiver.
    /// # Arguments
    ///
    /// * `since_tick` - The tick to diff from. Is None if the full state should be serialized.
    ///
    fn serialize<'w, 's>(
        &self,
        param: &<<Self::Param as SystemParam>::Fetch as SystemParamFetch<'w, 's>>::Item,
        receiver: Option<ConnectionId>,
        since_tick: Option<NonZeroU32>,
    ) -> Option<Bytes>;

    /// Updates the internal change state.
    /// Returns true if changed this tick.
    fn update_state(&mut self, tick: u32) -> bool;

    /// The priority of this component. The higher the priority, the sooner will it be sent under congestion.
    fn priority(&self) -> i16 {
        0i16
    }
}

/// A trait implemented by any component that receives network updates from the server.
pub trait NetworkedFromServer: TypeUuid + Sized {
    type Param: SystemParam;

    fn deserialize<'w, 's>(
        &mut self,
        param: &<<Self::Param as SystemParam>::Fetch as SystemParamFetch<'w, 's>>::Item,
        data: &[u8],
    );

    /// The initial value used if the component is not already present.
    /// Returns `None` if the component should not be added automatically.
    fn default_if_missing() -> Option<Self>;
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

    pub fn update_state(&mut self, tick: u32) -> bool {
        if matches!(self.change_state, ChangeState::Dirty) {
            self.change_state = ChangeState::Clean {
                last_changed_tick: tick,
            };
            true
        } else {
            false
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
        self.change_state = ChangeState::Dirty;
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
    // This is an Option because the data is inserted into the world before we can set the value from the server.
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

/// Maps uuids to a smaller data type to save network bandwith.
pub(crate) struct NetworkRegistry<T> {
    entries: Vec<Uuid>,
    phantom: PhantomData<T>,
}

impl<T> Default for NetworkRegistry<T> {
    fn default() -> Self {
        Self {
            entries: Default::default(),
            phantom: Default::default(),
        }
    }
}

impl<T: Into<u16> + From<u16>> NetworkRegistry<T> {
    pub(crate) fn register<K: NetworkedFromServer>(&mut self) -> bool {
        if self.entries.len() >= u16::MAX as usize {
            panic!(
                "Too many different {} registered.",
                std::any::type_name::<T>()
            );
        }

        let uuid = K::TYPE_UUID;
        // Components must be sorted by UUID so the index is always the same
        if let Err(pos) = self.entries.binary_search(&uuid) {
            self.entries.insert(pos, uuid);
            return true;
        }
        false
    }

    pub(crate) fn get_id(&self, uuid: &Uuid) -> Option<T> {
        self.entries
            .binary_search(uuid)
            .ok()
            .map(|i| (i as u16).into())
    }

    pub(crate) fn get_uuid(&self, id: T) -> Option<&Uuid> {
        self.entries.get(id.into() as usize)
    }
}
