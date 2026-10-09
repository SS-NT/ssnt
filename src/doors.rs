use bevy::prelude::*;
use bevy_rapier3d::prelude::{
    AdditionalMassProperties, Collider, CollisionGroups, ExternalForce, Group, ImpulseJoint,
    PrismaticJointBuilder, QueryFilter, ReadMassProperties, ReadRapierContext, RigidBody, Velocity,
};
use networking::is_server;
use physics::{PhysicsEntityCommands, CHARACTER_GROUP, DEFAULT_GROUP};

pub struct DoorPlugin;

impl Plugin for DoorPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Door>().register_type::<DoorPanel>();

        if is_server(app) {
            app.add_systems(
                Update,
                (
                    configure_door_panels,
                    detect_door_users,
                    move_door_panels,
                    apply_door_panel_force,
                )
                    .chain(),
            );
        }
    }
}

/// Distance (metres) from the target position at which a panel counts as arrived
const ARRIVE_DISTANCE: f32 = 0.01;
/// Speed (m/s) a body other than a character needs to open a door
const MIN_OPENING_SPEED: f32 = 0.2;
/// Speed (m/s) below which a panel counts as standing still
const STILL_SPEED: f32 = 0.05;
/// Seconds after a panel starts moving before it can count as stuck
const STUCK_GRACE_SECONDS: f32 = 0.3;
/// How quickly a panel slows down in the last few centimetres before its target (1/s)
const BRAKING_GAIN: f32 = 20.0;
/// Share of the maximum force used to plan braking, the rest corrects for lag
const BRAKING_FORCE_SHARE: f32 = 0.7;

/// A door that opens automatically when a sufficiently large body comes near.
/// Its sliding panel is a child with a [`DoorPanel`].
/// The panel is only simulated on the server, clients follow its networked transform.
#[derive(Component, Reflect)]
#[reflect(Component, Default)]
pub struct Door {
    /// How far in front of and behind the door bodies are detected
    detection_range: f32,
    /// Minimum collider volume (m³) of a body to open the door
    min_volume: f32,
    /// Seconds the door stays open after a body was last detected
    stay_open_seconds: f32,
    /// Seconds a closing panel may be stuck before the door opens again
    stuck_seconds: f32,

    #[reflect(ignore)]
    open: bool,
    #[reflect(ignore)]
    last_detected: f32,
    #[reflect(ignore)]
    move_started: f32,
    #[reflect(ignore)]
    stuck_since: Option<f32>,
}

impl Default for Door {
    fn default() -> Self {
        Self {
            detection_range: 1.5,
            min_volume: 0.05,
            stay_open_seconds: 1.5,
            stuck_seconds: 0.5,
            open: false,
            last_detected: 0.0,
            move_started: 0.0,
            stuck_since: None,
        }
    }
}

impl Door {
    fn set_open(&mut self, open: bool, now: f32) {
        if self.open != open {
            self.open = open;
            self.move_started = now;
            self.stuck_since = None;
        }
    }
}

/// The sliding part of a [`Door`]. It moves along the door's local x axis, starting at the origin when closed.
#[derive(Component, Reflect)]
#[reflect(Component, Default)]
pub struct DoorPanel {
    /// Local x position when fully open
    open_offset: f32,
    /// Maximum sliding speed (m/s)
    speed: f32,
    /// Maximum force the door can push with
    max_force: f32,
    /// Mass added to the mass of the panel's colliders
    mass: f32,
}

impl Default for DoorPanel {
    fn default() -> Self {
        Self {
            open_offset: -0.96,
            speed: 2.0,
            max_force: 5.0,
            mass: 0.5,
        }
    }
}

impl DoorPanel {
    fn target(&self, open: bool) -> f32 {
        if open {
            self.open_offset
        } else {
            0.0
        }
    }
}

/// Attaches panels to their door with a sliding joint and sets their mass
fn configure_door_panels(
    mut commands: Commands,
    panels: Query<(Entity, &DoorPanel, &ChildOf), Added<DoorPanel>>,
    bodies: Query<(), With<RigidBody>>,
) {
    for (entity, panel, child_of) in panels.iter() {
        let door = child_of.parent();
        if !bodies.contains(door) {
            commands.entity(door).insert(RigidBody::Fixed);
        }

        let joint = PrismaticJointBuilder::new(Vec3::X)
            .limits([panel.open_offset.min(0.0), panel.open_offset.max(0.0)]);
        commands.entity(entity).insert((
            AdditionalMassProperties::Mass(panel.mass),
            ImpulseJoint::new(door, joint),
        ));
    }
}

fn detect_door_users(
    mut doors: Query<(&mut Door, &GlobalTransform)>,
    colliders: Query<(&Collider, Option<&CollisionGroups>)>,
    parents: Query<&ChildOf>,
    velocities: Query<&Velocity>,
    rapier: ReadRapierContext,
    time: Res<Time>,
) {
    let Ok(context) = rapier.single() else {
        return;
    };
    let now = time.elapsed_secs();
    // Client controlled bodies are kinematic on the server
    let filter = QueryFilter::exclude_fixed().groups(CollisionGroups::new(
        Group::ALL,
        DEFAULT_GROUP | CHARACTER_GROUP,
    ));

    for (mut door, transform) in doors.iter_mut() {
        let area = Collider::cuboid(0.5, 0.9, door.detection_range);
        let mut detected = false;
        context.intersect_shape(
            transform.transform_point(Vec3::new(0.0, 1.0, 0.0)),
            transform.rotation(),
            &*area.raw,
            filter,
            |entity| {
                let Ok((collider, groups)) = colliders.get(entity) else {
                    return true;
                };
                if collider.raw.mass_properties(1.0).mass() < door.min_volume {
                    return true;
                }
                // Characters keep the door open while standing in it, everything else has to move
                let is_character = groups.is_some_and(|g| g.memberships.contains(CHARACTER_GROUP));
                let is_moving = || {
                    std::iter::once(entity)
                        .chain(parents.iter_ancestors(entity))
                        .find_map(|e| velocities.get(e).ok())
                        .is_some_and(|v| v.linear.length() > MIN_OPENING_SPEED)
                };
                detected = is_character || is_moving();
                !detected
            },
        );

        if detected {
            door.last_detected = now;
            door.set_open(true, now);
        } else if door.open && now - door.last_detected > door.stay_open_seconds {
            door.set_open(false, now);
        }
    }
}

/// Starts and stops the panel simulation and reopens doors that are stuck while closing
fn move_door_panels(
    mut doors: Query<(&mut Door, &Children)>,
    mut panels: Query<(
        &DoorPanel,
        &mut Transform,
        &mut Velocity,
        &GlobalTransform,
        Has<physics::Frozen>,
    )>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    for (mut door, children) in doors.iter_mut() {
        for &child in children {
            let Ok((panel, mut transform, mut velocity, global, frozen)) = panels.get_mut(child)
            else {
                continue;
            };

            let target = panel.target(door.open);
            let distance = target - transform.translation.x;
            if frozen {
                if distance.abs() > ARRIVE_DISTANCE {
                    commands.entity(child).unfreeze(None);
                    door.move_started = now;
                    door.stuck_since = None;
                }
                continue;
            }

            let speed = velocity.linear.dot(global.rotation() * Vec3::X);
            if distance.abs() <= ARRIVE_DISTANCE && speed.abs() < STILL_SPEED {
                transform.translation = Vec3::X * target;
                *velocity = Velocity::zero();
                commands.entity(child).freeze(None);
                continue;
            }

            if !door.open
                && speed.abs() < STILL_SPEED
                && now - door.move_started > STUCK_GRACE_SECONDS
            {
                let stuck_since = *door.stuck_since.get_or_insert(now);
                if now - stuck_since > door.stuck_seconds {
                    door.last_detected = now;
                    door.set_open(true, now);
                }
            } else {
                door.stuck_since = None;
            }
        }
    }
}

/// Pushes simulated panels towards their open or closed position
fn apply_door_panel_force(
    doors: Query<&Door>,
    mut panels: Query<(
        Entity,
        &DoorPanel,
        &ChildOf,
        &Transform,
        &GlobalTransform,
        &Velocity,
        &ReadMassProperties,
        &RigidBody,
        Option<&mut ExternalForce>,
    )>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().max(f32::EPSILON);
    for (entity, panel, child_of, transform, global, velocity, mass, body, force) in
        panels.iter_mut()
    {
        let new_force = match (body, doors.get(child_of.parent())) {
            (RigidBody::Dynamic, Ok(door)) => {
                let axis = global.rotation() * Vec3::X;
                let distance = panel.target(door.open) - transform.translation.x;
                let mass = mass.get().mass;
                let deceleration = BRAKING_FORCE_SHARE * panel.max_force / mass;
                let braking_speed = (2.0 * deceleration * distance.abs())
                    .sqrt()
                    .min(distance.abs() * BRAKING_GAIN);
                let wanted_speed = distance.signum() * panel.speed.min(braking_speed);
                let speed = velocity.linear.dot(axis);
                let magnitude =
                    ((wanted_speed - speed) * mass / dt).clamp(-panel.max_force, panel.max_force);
                axis * magnitude
            }
            _ => Vec3::ZERO,
        };

        match force {
            Some(mut force) => {
                if force.force != new_force {
                    force.force = new_force;
                }
            }
            None => {
                commands.entity(entity).insert(ExternalForce {
                    force: new_force,
                    ..Default::default()
                });
            }
        }
    }
}
