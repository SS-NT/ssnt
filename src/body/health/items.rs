use std::time::Duration;

use bevy::prelude::*;
use networking::is_server;

use crate::{
    body::Body,
    interaction::{
        ActiveInteraction, GenerateInteractionList, InteractionListEvents, InteractionOption,
        InteractionSpecificity, InteractionStatus,
    },
};

use super::{OrganicBody, OrganicBodyPart, OrganicBrain, OrganicHeart, OrganicLaceration};

pub struct HealthItemsPlugin;

impl Plugin for HealthItemsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<HealingItem>()
            .register_type::<HealOrganicLaceration>()
            .register_type::<BloodTransfusion>()
            .register_type::<Defibrillator>();

        if is_server(app) {
            app.register_type::<ApplyMedicineInteraction>()
                .register_type::<TransfuseInteraction>()
                .register_type::<DefibrillateInteraction>()
                .add_systems(
                    Update,
                    (
                        apply_medicine_interaction,
                        prepare_transfusion_interaction.in_set(GenerateInteractionList),
                        transfusion_interaction,
                        prepare_defibrillate_interaction.in_set(GenerateInteractionList),
                        defibrillate_interaction,
                    ),
                );
        }
    }
}

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct HealingItem;

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
struct HealOrganicLaceration {}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
pub struct ApplyMedicineInteraction {
    pub medicine: Entity,
    pub wound: Entity,
}

impl FromWorld for ApplyMedicineInteraction {
    fn from_world(_: &mut World) -> Self {
        Self {
            medicine: Entity::PLACEHOLDER,
            wound: Entity::PLACEHOLDER,
        }
    }
}

fn apply_medicine_interaction(
    mut query: Query<(&mut ApplyMedicineInteraction, &mut ActiveInteraction)>,
    medicines: Query<AnyOf<(&HealOrganicLaceration,)>>,
    wounds: Query<AnyOf<(&OrganicLaceration,)>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for (interaction, mut active) in query.iter_mut() {
        active.set_initial_duration(Duration::from_millis(1000));

        if active.start_time() + 1.0 > time.elapsed_seconds() {
            continue;
        }

        let Ok((heal_organic_laceration,)) = medicines.get(interaction.medicine) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Ok((organic_laceration,)) = wounds.get(interaction.wound) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let mut heal = false;
        if organic_laceration.is_some() && heal_organic_laceration.is_some() {
            // TODO: Treating the wound shouldn't instantly make it disappear
            heal = true;
        }

        if heal {
            commands.entity(interaction.wound).despawn_recursive();
        }

        active.status = InteractionStatus::Completed;
    }
}

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
struct BloodTransfusion {}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct TransfuseInteraction {
    item: Entity,
}

impl FromWorld for TransfuseInteraction {
    fn from_world(_: &mut World) -> Self {
        Self {
            item: Entity::from_raw(0),
        }
    }
}

fn prepare_transfusion_interaction(
    interaction_list: Res<InteractionListEvents>,
    transfusions: Query<(), With<BloodTransfusion>>,
    bodies: Query<(), With<OrganicBody>>,
) {
    for event in interaction_list.events.iter() {
        if !bodies.contains(event.target) {
            continue;
        }

        let Some(item) = event.item_in_hand else {
            continue;
        };

        if !transfusions.contains(item) {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Transfuse blood".into(),
            interaction: Box::new(TransfuseInteraction { item }),
            specificity: InteractionSpecificity::Specific,
        });
    }
}

const TRANSFUSION_DURATION: Duration = Duration::from_millis(3000);

fn transfusion_interaction(
    mut query: Query<(&mut TransfuseInteraction, &mut ActiveInteraction)>,
    mut bodies: Query<(Entity, &mut OrganicBody)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for (interaction, mut active) in query.iter_mut() {
        let Ok((_, mut body)) = bodies.get_mut(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        active.set_initial_duration(TRANSFUSION_DURATION);

        if active.start_time() + TRANSFUSION_DURATION.as_secs_f32() > time.elapsed_seconds() {
            continue;
        }

        let capacity = body.blood_capacity;
        body.set_blood(capacity);

        commands.entity(interaction.item).despawn_recursive();
        active.status = InteractionStatus::Completed;
    }
}

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
struct Defibrillator {}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct DefibrillateInteraction {
    item: Entity,
}

impl FromWorld for DefibrillateInteraction {
    fn from_world(_: &mut World) -> Self {
        Self {
            item: Entity::from_raw(0),
        }
    }
}

fn prepare_defibrillate_interaction(
    interaction_list: Res<InteractionListEvents>,
    defibrillators: Query<(), With<Defibrillator>>,
    bodies: Query<&Body>,
    hearts: Query<(), With<OrganicHeart>>,
) {
    for event in interaction_list.events.iter() {
        let Some(item) = event.item_in_hand else {
            continue;
        };

        if !defibrillators.contains(item) {
            continue;
        }

        let Ok(body) = bodies.get(event.target) else {
            continue;
        };

        // Does target have any hearts?
        if hearts.iter_many(&body.limbs).next().is_none() {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Defibrillate".into(),
            interaction: Box::new(DefibrillateInteraction { item }),
            specificity: InteractionSpecificity::Specific,
        });
    }
}

const DEFIBRILLATE_DURATION: Duration = Duration::from_millis(5000);

fn defibrillate_interaction(
    mut query: Query<(&mut DefibrillateInteraction, &mut ActiveInteraction)>,
    bodies: Query<&Body>,
    mut organs: Query<(
        AnyOf<(&mut OrganicHeart, &OrganicBrain)>,
        Option<&mut OrganicBodyPart>,
    )>,
    time: Res<Time>,
) {
    for (_interaction, mut active) in query.iter_mut() {
        let Ok(body) = bodies.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        active.set_initial_duration(DEFIBRILLATE_DURATION);

        if active.start_time() + DEFIBRILLATE_DURATION.as_secs_f32() > time.elapsed_seconds() {
            continue;
        }

        for &limb in body.limbs.iter() {
            if let Ok(((heart, brain), mut body_part)) = organs.get_mut(limb) {
                if let Some(mut heart) = heart {
                    // Restart heart
                    if heart.heart_rate < 20 {
                        heart.heart_rate = 80;
                    } else {
                        // RIP lol
                        heart.heart_rate = 0;
                    }

                    // Refresh heart oxygen so it has a chance to pump
                    if let Some(body_part) = body_part.as_mut() {
                        body_part.refresh_oxygen(f32::MAX);
                    }
                }

                if let (Some(_), Some(mut body_part)) = (brain, body_part) {
                    // Heal brain damage
                    // I know this makes no sense, sue me
                    body_part.integrity = 1.0;
                }
            }
        }

        active.status = InteractionStatus::Completed;
    }
}
