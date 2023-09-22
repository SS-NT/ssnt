//! Provides abstractions for submitting work to gameplay systems and retrieving results.

use std::{hash::Hash, marker::PhantomData, num::NonZeroU32};

use bevy::prelude::*;
use bevy::utils::HashMap;

/// Trait for any struct that can be submitted as a task.
pub trait Task {
    type Result;
}

/// A resource storing tasks of a specific type.
#[derive(Resource)]
pub struct Tasks<T: Task> {
    pending: Vec<PendingTask<T>>,
    completed: HashMap<TaskId<T>, T::Result>,
    next_id: NonZeroU32,
}

impl<T: Task> Default for Tasks<T> {
    fn default() -> Self {
        Self {
            pending: Default::default(),
            completed: Default::default(),
            next_id: NonZeroU32::new(1).unwrap(),
        }
    }
}

impl<T: Task + 'static> Tasks<T> {
    fn next_id(&mut self) -> TaskId<T> {
        let id = TaskId {
            id: self.next_id,
            phantom: Default::default(),
        };
        self.next_id = self.next_id.checked_add(1).unwrap();
        id
    }

    /// Create a task. The returned id can be used to retrieve the result later.
    #[must_use = "Task results are kept until they are retrieved. If you don't care about the result, use `create_ignore` instead"]
    pub fn create(&mut self, task: T) -> TaskId<T> {
        let id = self.next_id();
        self.pending.push(PendingTask {
            id,
            data: task,
            keep_result: true,
        });
        id
    }

    /// Create a task, ignoring the result.
    pub fn create_ignore(&mut self, task: T) {
        let id = self.next_id();
        self.pending.push(PendingTask {
            id,
            data: task,
            keep_result: false,
        });
    }

    /// Fulfill all pending tasks.
    pub fn process(&mut self, mut f: impl FnMut(T) -> T::Result) {
        for task in self.pending.drain(..) {
            let result = f(task.data);
            if task.keep_result {
                self.completed.insert(task.id, result);
                if self.completed.len() > 10000 {
                    warn!(
                        "Many unused results for {} task",
                        std::any::type_name::<T>()
                    );
                }
            }
        }
    }

    /// Try to fulfill pending tasks.
    pub fn try_process<S: Default>(
        &mut self,
        state: &mut HashMap<TaskId<T>, S>,
        mut f: impl FnMut(&T, &mut S) -> TaskStatus<T::Result>,
    ) {
        self.pending.retain(|task| {
            let result = f(
                &task.data,
                state.entry(task.id).or_insert_with(Default::default),
            );
            match result {
                TaskStatus::Done(t) => {
                    state.remove(&task.id);
                    if task.keep_result {
                        self.completed.insert(task.id, t);
                        if self.completed.len() > 10000 {
                            warn!(
                                "Many unused results for {} task",
                                std::any::type_name::<T>()
                            );
                        }
                    }
                    false
                }
                TaskStatus::Pending => true,
            }
        });
    }

    /// Retrieve the result for a task. Returns None if the task hasn't been completed.
    pub fn result(&mut self, id: TaskId<T>) -> Option<T::Result> {
        self.completed.remove(&id)
    }
}

struct PendingTask<T> {
    id: TaskId<T>,
    data: T,
    keep_result: bool,
}

pub struct TaskId<T> {
    id: NonZeroU32,
    phantom: PhantomData<fn() -> T>,
}

impl<T: 'static> Hash for TaskId<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T> Copy for TaskId<T> {}
impl<T> Clone for TaskId<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Eq for TaskId<T> {}
impl<T> PartialEq for TaskId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

pub enum TaskStatus<T> {
    Done(T),
    Pending,
}
