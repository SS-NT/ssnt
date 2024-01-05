use bevy::{ecs::query::Has, prelude::*};
use networking::is_server;

use crate::combat::damage::*;

use super::Body;

mod items;
mod scanner;
mod ui;

pub struct HealthPlugin;

impl Plugin for HealthPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<OrganicBody>()
            .register_type::<OrganicBodyPart>()
            .register_type::<OrganicLung>()
            .register_type::<OrganicHeart>()
            .register_type::<OrganicBrain>();
        if is_server(app) {
            app.add_event::<HeartBeat>()
                .add_event::<BrainStateEvent>()
                .add_systems(
                    Update,
                    (
                        (heart_beat, adjust_heart_rate).chain(),
                        breathing,
                        lung_gas_exchange,
                        receive_damage,
                        brain_live,
                    ),
                );
        }
        app.add_plugins((
            scanner::HealthScannerPlugin,
            items::HealthItemsPlugin,
            ui::HealthUiPlugin,
        ));
    }
}

/// How many liters of oxygen can fit in a liter of blood
const MAX_BLOOD_OXYGEN: f32 = 0.05;

#[derive(Component, Reflect)]
#[reflect(Component)]
struct OrganicBody {
    /// Amount of blood in liters
    blood: f32,
    /// Maximum amount of blood that can be retained
    blood_capacity: f32,
    /// Amount of oxygen in blood in liters
    oxygen_in_blood: f32,
}

impl Default for OrganicBody {
    fn default() -> Self {
        let blood_capacity = 5.0;
        Self {
            blood: blood_capacity,
            blood_capacity,
            oxygen_in_blood: blood_capacity * MAX_BLOOD_OXYGEN,
        }
    }
}

impl OrganicBody {
    fn oxygen_capacity(&self) -> f32 {
        self.blood * MAX_BLOOD_OXYGEN
    }

    fn add_oxygen(&mut self, amount: f32) -> f32 {
        let consumed = (self.oxygen_capacity() - self.oxygen_in_blood).clamp(0.0, amount);
        self.oxygen_in_blood += consumed;
        consumed
    }

    fn set_blood(&mut self, amount: f32) {
        self.blood = amount;
        self.oxygen_in_blood = self.oxygen_in_blood.min(self.oxygen_capacity());
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
struct OrganicBodyPart {
    /// How much oxygen this body part has consumed.
    /// This is reduced when oxygen is provided through the blood.
    oxygen_consumed: f32,
    /// How much oxygen this body part can retain.
    /// `oxygen_consumed` will max out at this value.
    oxygen_capacity: f32,
    /// How damaged the body part is.
    /// 1 is fully capable, 0 is unusable
    integrity: f32,
}

impl FromWorld for OrganicBodyPart {
    fn from_world(_: &mut World) -> Self {
        Self {
            oxygen_consumed: 0.0,
            oxygen_capacity: 0.0015,
            integrity: 1.0,
        }
    }
}

impl OrganicBodyPart {
    fn oxygen_remaining(&self) -> f32 {
        self.oxygen_capacity - self.oxygen_consumed
    }

    fn oxygen_saturation(&self) -> f32 {
        self.oxygen_remaining() / self.oxygen_capacity
    }

    fn consume_oxygen(&mut self, amount: f32) -> f32 {
        let consumed = amount.min(self.oxygen_remaining());
        self.oxygen_consumed += consumed;
        consumed
    }

    fn refresh_oxygen(&mut self, amount: f32) -> f32 {
        let consumed = amount.min(self.oxygen_consumed);
        self.oxygen_consumed -= consumed;
        consumed
    }

    fn damage(&mut self, amount: f32) {
        self.integrity = (self.integrity - amount).max(0.0)
    }

    fn unusable(&self) -> bool {
        self.integrity <= 0.01
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
struct OrganicHeart {
    /// How much blood is pumped per beat in liters per second
    pump_rate: f32,
    /// Beats per minute
    heart_rate: u32,
    last_beat: f32,
}

impl Default for OrganicHeart {
    fn default() -> Self {
        Self {
            pump_rate: 0.070,
            heart_rate: 70,
            last_beat: 0.0,
        }
    }
}

#[derive(Event)]
struct HeartBeat {
    body: Entity,
    blood_amount: f32,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
struct OrganicLung {
    /// Air capacity of the lung in liters
    capacity: f32,
    /// How much oxygen is in the lungs in liters
    // TODO: Change this to be a proper gas container
    oxygen_present: f32,
    /// How much gas is exchanged per liter of blood in a heartbeat
    exchange_rate: f32,
    /// Breaths per minute
    breath_rate: u32,
    last_breath: f32,
}

impl Default for OrganicLung {
    fn default() -> Self {
        Self {
            capacity: 6.0,
            // Full breath of oxygen
            oxygen_present: 6.0 * 0.21,
            exchange_rate: 4.28,
            breath_rate: 12,
            last_breath: 0.0,
        }
    }
}

/// Oxygen supply to the brain will be averaged over this many seconds.
const BRAIN_OXYGEN_AVERAGE_WINDOW: f32 = 4.0;
const BRAIN_OXYGEN_LEN: usize = (BRAIN_OXYGEN_AVERAGE_WINDOW / BRAIN_UPDATE_INTERVAL) as usize;

#[derive(Component, Reflect)]
#[reflect(Component)]
struct OrganicBrain {
    low_blood: bool,
    unconcious: bool,

    last_think: f32,

    last_oxygen_ratios: [f32; BRAIN_OXYGEN_LEN],
    oxygen_history_index: usize,
}

impl Default for OrganicBrain {
    fn default() -> Self {
        Self {
            low_blood: Default::default(),
            unconcious: Default::default(),
            last_think: Default::default(),
            last_oxygen_ratios: [1.0; BRAIN_OXYGEN_LEN],
            oxygen_history_index: Default::default(),
        }
    }
}

impl OrganicBrain {
    fn oxygen_ratio(&self) -> f32 {
        let sum: f32 = self.last_oxygen_ratios.iter().sum();
        let count: f32 = self.last_oxygen_ratios.len() as f32;
        sum / count
    }

    fn add_oxygen_ratio(&mut self, new_ratio: f32) {
        self.last_oxygen_ratios[self.oxygen_history_index] = new_ratio;
        self.oxygen_history_index = (self.oxygen_history_index + 1) % self.last_oxygen_ratios.len();
    }
}

#[derive(Event)]
pub struct BrainStateEvent {
    pub brain: Entity,
    pub new_state: BrainState,
}

#[derive(PartialEq, Eq)]
pub enum BrainState {
    Conscious,
    Unconscious,
    Dead,
}

fn adjust_heart_rate(
    mut hearts: Query<(Entity, &mut OrganicHeart, Option<&OrganicBodyPart>)>,
    bodies: Query<&Body>,
    body_parts: Query<(&OrganicBodyPart, Has<OrganicBrain>)>,
    parents: Query<&Parent>,
) {
    'outer: for (heart_entity, mut heart, heart_part) in hearts.iter_mut() {
        // Do not adjust heart rate if in cardiac arrest
        if heart.heart_rate == 0 {
            continue;
        }

        let Some(body_entity) = parents
            .iter_ancestors(heart_entity)
            .find(|e| bodies.contains(*e))
        else {
            continue;
        };
        let body = bodies.get(body_entity).unwrap();

        let mut average_oxygen = 0.0;
        let mut average_count = 0;
        let mut iter = body_parts.iter_many(&body.limbs);
        while let Some((part, is_brain)) = iter.fetch_next() {
            average_count += 1;
            average_oxygen += part.oxygen_saturation();

            // No heart beat if braindead
            if is_brain && part.unusable() {
                heart.heart_rate = 0;
                continue 'outer;
            }
        }
        average_oxygen /= average_count as f32;

        heart.heart_rate = (RESTING_HEART_BPM as f32 * average_oxygen
            + INTENSE_HEART_BPM as f32 * (1.0 - average_oxygen)) as u32;

        // Heart beats slower if it runs low on oxygen
        let heart_oxygen = heart_part.map(|h| h.oxygen_saturation()).unwrap_or(1.0);
        heart.heart_rate = (heart.heart_rate as f32 * heart_oxygen) as u32;
    }
}

const RESTING_HEART_CONSUMPTION: f32 = 0.00034;
const RESTING_HEART_BPM: u32 = 70;
const INTENSE_HEART_CONSUMPTION: f32 = 0.0015;
const INTENSE_HEART_BPM: u32 = 200;

fn heart_beat(
    mut hearts: Query<(Entity, &mut OrganicHeart)>,
    mut bodies: Query<(&Body, &mut OrganicBody)>,
    mut body_parts: Query<(&Parent, &mut OrganicBodyPart)>,
    mut event: EventWriter<HeartBeat>,
    lacerations: Query<(&OrganicLaceration, &Parent)>,
    parents: Query<&Parent>,
    time: Res<Time>,
) {
    for (heart_entity, mut heart) in hearts.iter_mut() {
        // Is it time for the heart to beat again?
        if heart.last_beat + (60.0 / heart.heart_rate as f32) > time.elapsed_seconds() {
            continue;
        }

        heart.last_beat = time.elapsed_seconds();

        // The heart consumes oxygen to beat
        let mut pump_strength = 1.0;
        if let Ok((_, mut heart_part)) = body_parts.get_mut(heart_entity) {
            // Calculate oxygen needed per beat depending on BPM
            // Higher BPM gives the heart muscles less time to relax, reducing their efficiency
            let t = (heart.heart_rate.saturating_sub(RESTING_HEART_BPM) as f32
                / (INTENSE_HEART_BPM - RESTING_HEART_BPM) as f32)
                .clamp(0.0, 1.0);
            // TODO: Linear interpolation is not really accurate
            let desired_oxygen =
                (1.0 - t) * RESTING_HEART_CONSUMPTION + t * INTENSE_HEART_CONSUMPTION;
            let consumed = heart_part.consume_oxygen(desired_oxygen);
            let ratio = consumed / desired_oxygen;
            bevy::log::debug!(
                "Heart beat {}/{} ({}%)",
                consumed,
                desired_oxygen,
                ratio * 100.0
            );
            if ratio < 0.1 {
                // The heart doesn't have enough oxygen and stops
                heart.heart_rate = 0;
                bevy::log::warn!("CARDIAC ARREST");
                continue;
            }
            pump_strength = ratio;
        };

        let Some(body_entity) = parents
            .iter_ancestors(heart_entity)
            .find(|e| bodies.contains(*e))
        else {
            continue;
        };

        let (body, mut organic_body) = bodies.get_mut(body_entity).unwrap();

        // How much blood we pump depends on how well the heart works and how much blood is in the body
        let body_blood_ratio = organic_body.blood / organic_body.blood_capacity;
        let blood_pressure = body_blood_ratio * 2.0 - 1.0;
        let blood_to_spread = heart.pump_rate * pump_strength * blood_pressure;
        event.send(HeartBeat {
            body: body_entity,
            blood_amount: blood_to_spread,
        });
        let blood_oxygen_saturation = organic_body.oxygen_in_blood / organic_body.blood;
        let mut oxygen_to_spread = blood_to_spread * blood_oxygen_saturation;

        // Heart gets refreshed with new blood before all other parts
        if let Ok((_, mut heart_part)) = body_parts.get_mut(heart_entity) {
            let heart_oxygen_used = heart_part.refresh_oxygen(oxygen_to_spread);
            oxygen_to_spread -= heart_oxygen_used;
            organic_body.oxygen_in_blood -= heart_oxygen_used;
        }

        // Provide blood to other parts
        let parts_count = body_parts.iter_many(&body.limbs).count();
        if parts_count == 0 {
            continue;
        }

        let oxygen_per_part = oxygen_to_spread / parts_count as f32;
        let mut oxygen_consumed = 0.0;
        let mut iter = body_parts.iter_many_mut(&body.limbs);
        while let Some((_, mut part)) = iter.fetch_next() {
            oxygen_consumed += part.refresh_oxygen(oxygen_per_part);
        }

        organic_body.oxygen_in_blood -= oxygen_consumed;

        bevy::log::debug!(
            "Blood pumped {}, blood total {}, saturation {}, oxygen total {}, oxygen per part {}",
            blood_to_spread,
            organic_body.blood,
            blood_oxygen_saturation,
            oxygen_to_spread,
            oxygen_per_part
        );

        for (laceration, parent) in lacerations.iter() {
            let Ok((body_part_parent, _)) = body_parts.get(parent.get()) else {
                continue;
            };

            // TODO: Can we make this more efficient?
            if body_entity != body_part_parent.get() {
                continue;
            }

            let current_blood = organic_body.blood;
            organic_body
                .set_blood(current_blood - laceration.size.blood_loss_ratio() * blood_to_spread);
        }
    }
}

const LUNG_CONSUMPTION: f32 = 0.0004;

fn breathing(mut lungs: Query<(&mut OrganicLung, Option<&mut OrganicBodyPart>)>, time: Res<Time>) {
    for (mut lung, part) in lungs.iter_mut() {
        // Is it time for the next breath
        if lung.last_breath + (60.0 / lung.breath_rate as f32) > time.elapsed_seconds() {
            continue;
        }

        lung.last_breath = time.elapsed_seconds();

        // Lung consumes oxygen to work
        let mut breath_strength = 1.0;
        if let Some(mut part) = part {
            let consumed = part.consume_oxygen(LUNG_CONSUMPTION);
            let ratio = consumed / LUNG_CONSUMPTION;
            breath_strength = ratio;
            if breath_strength < 0.05 {
                continue;
            }
        };

        // TODO: replace with gas content in air
        // We breathe a full lung of air (21% is oxygen)
        lung.oxygen_present = lung.capacity * breath_strength * 0.21;
    }
}

fn lung_gas_exchange(
    mut lungs: Query<(Entity, &mut OrganicLung)>,
    mut bodies: Query<(&Body, &mut OrganicBody)>,
    mut beats: EventReader<HeartBeat>,
) {
    for beat in beats.iter() {
        let Ok((body, mut organic_body)) = bodies.get_mut(beat.body) else {
            continue;
        };
        let mut iter = lungs.iter_many_mut(&body.limbs);
        while let Some((_, mut lung)) = iter.fetch_next() {
            if lung.oxygen_present < 0.001 {
                continue;
            }

            let oxygen_to_exchange =
                (lung.exchange_rate * beat.blood_amount).min(lung.oxygen_present);
            let oxygen_exchanged = organic_body.add_oxygen(oxygen_to_exchange);
            lung.oxygen_present -= oxygen_exchanged;
        }
    }
}

/// Oxygen consumption of the brain per second
const BRAIN_CONSUMPTION: f32 = 0.00081;
const BRAIN_UPDATE_INTERVAL: f32 = 0.2;

fn brain_live(
    mut brains: Query<(Entity, &mut OrganicBrain, Option<&mut OrganicBodyPart>)>,
    mut state_events: EventWriter<BrainStateEvent>,
    time: Res<Time>,
) {
    for (brain_entity, mut brain, part) in brains.iter_mut() {
        // Braindead... lol
        if part.as_ref().map(|p| p.unusable()).unwrap_or_default() {
            continue;
        }
        // Not time to think yet
        if brain.last_think + BRAIN_UPDATE_INTERVAL > time.elapsed_seconds() {
            continue;
        }
        let pondering_time = time.elapsed_seconds() - brain.last_think;
        brain.last_think = time.elapsed_seconds();

        // Brain consumes oxygen to work
        if let Some(mut part) = part {
            let to_consume = BRAIN_CONSUMPTION * pondering_time;
            let consumed = part.consume_oxygen(to_consume);
            let ratio = consumed / to_consume;

            brain.add_oxygen_ratio(ratio);
            let oxygen_average = brain.oxygen_ratio();

            let now_low_blood = oxygen_average < 0.7;
            if now_low_blood != brain.low_blood {
                brain.low_blood = now_low_blood;
            }

            let now_unconcious = oxygen_average < 0.2;
            if now_unconcious != brain.unconcious {
                brain.unconcious = now_unconcious;
                state_events.send(BrainStateEvent {
                    brain: brain_entity,
                    new_state: if now_unconcious {
                        BrainState::Unconscious
                    } else {
                        BrainState::Conscious
                    },
                });
            }

            // Cause brain damage / brain death on low oxygen
            if oxygen_average < 0.05 {
                part.damage(pondering_time * 0.1);
                if part.unusable() {
                    state_events.send(BrainStateEvent {
                        brain: brain_entity,
                        new_state: BrainState::Dead,
                    });
                }
            }
        } else {
            if brain.unconcious {
                brain.unconcious = false;
            }
            if brain.low_blood {
                brain.low_blood = false;
            }
        };
    }
}

#[derive(Component)]
struct OrganicLaceration {
    //    /// How much blood can exit the wound in liters per second
    // blood_leak_rate: f32,
    size: LacerationSize,
}

#[allow(dead_code)]
enum LacerationSize {
    Small,
    Medium,
    Large,
}

impl LacerationSize {
    fn blood_loss_ratio(&self) -> f32 {
        match self {
            LacerationSize::Small => 0.05,
            LacerationSize::Medium => 0.20,
            LacerationSize::Large => 0.40,
        }
    }
}

impl std::fmt::Display for LacerationSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            LacerationSize::Small => write!(f, "Small"),
            LacerationSize::Medium => write!(f, "Medium"),
            LacerationSize::Large => write!(f, "Large"),
        }
    }
}

fn receive_damage(
    attacks: Query<(Entity, &AffectedEntity, &KineticDamage), Added<Attack>>,
    body_parts: Query<&OrganicBodyPart>,
    mut commands: Commands,
) {
    for (attack_entity, affected_entity, _kinetic) in attacks.iter() {
        let Ok(_) = body_parts.get(affected_entity.0) else {
            continue;
        };

        bevy::log::debug!("Received wound");
        // TODO: Clothing/armor, hitting organs, arteries
        commands.entity(attack_entity).despawn();
        commands
            .spawn(OrganicLaceration {
                // TODO: Consider kinetic profile
                size: LacerationSize::Medium,
            })
            .set_parent(affected_entity.0);
    }
}
