use std::time::Duration;

use bevy::prelude::*;
use maps::MapCommandsExt;
use networking::is_server;

use crate::{
    event::{EventSystemExt, InterceptableEvents},
    interaction::{
        ActiveInteraction, InteractionListEvent, InteractionOption, InteractionSpecificity,
        InteractionStatus,
    },
};

pub struct ConstructionPlugin;

impl Plugin for ConstructionPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Wrench>()
            .register_type::<WrenchDeconstructable>()
            .register_type::<WrenchDeconstructInteraction>();
        if is_server(app) {
            app.add_system(
                prepare_deconstruct_wrench_interaction
                    .into_descriptor()
                    .intercept::<InteractionListEvent>(),
            )
            .add_system(execute_deconstruct_wrench_interaction);
        }
    }
}

const DECONSTRUCT_TIME: Duration = Duration::from_secs(2);

/// Marks an object as a wrench tool.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Wrench;

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct WrenchDeconstructable;

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct WrenchDeconstructInteraction {
    target: Entity,
}

// Dummy default for Reflect
impl Default for WrenchDeconstructInteraction {
    fn default() -> Self {
        Self {
            target: Entity::from_raw(0),
        }
    }
}

fn prepare_deconstruct_wrench_interaction(
    events: Res<InterceptableEvents<InteractionListEvent>>,
    wrenches: Query<(), With<Wrench>>,
    deconstructables: Query<(), With<WrenchDeconstructable>>,
) {
    for event in events.iter() {
        let Some(item_in_hand) = event.item_in_hand else {
            continue;
        };

        if !wrenches.contains(item_in_hand) {
            continue;
        }

        if !deconstructables.contains(event.target) {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Deconstruct".into(),
            interaction: Box::new(WrenchDeconstructInteraction {
                target: event.target,
            }),
            specificity: InteractionSpecificity::Specific,
        });
    }
}

fn execute_deconstruct_wrench_interaction(
    mut query: Query<(&WrenchDeconstructInteraction, &mut ActiveInteraction)>,
    deconstructables: Query<(), With<WrenchDeconstructable>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for (interaction, mut active) in query.iter_mut() {
        active.set_initial_duration(DECONSTRUCT_TIME);

        if !deconstructables.contains(active.target) {
            active.status = InteractionStatus::Canceled;
            continue;
        }

        if active.start_time() + DECONSTRUCT_TIME.as_secs_f32() > time.elapsed_seconds() {
            continue;
        }

        commands.despawn_tile_entity(interaction.target);
        active.status = InteractionStatus::Completed;
    }
}
