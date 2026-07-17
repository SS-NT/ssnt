//! Reproduction of the island-manager panic seen when "cutting" a body into limbs:
//!
//!   assertion failed: rb2.is_fixed() || rb2.ids.active_island_id != usize::MAX
//!   (rapier3d island_manager/manager.rs, via narrow_phase compute_contacts)
//!
//! It mirrors the game flow:
//!   1. spawn a creature: a dynamic root body (with a child capsule collider) plus a
//!      nested tree of dynamic-body limbs, each with its own collider,
//!   2. freeze every limb (`freeze(AttachedLimbs)` in `process_new_limbs`), then
//!   3. cut: `disable_physics(root)` followed by, for each limb,
//!      `remove::<ChildOf>().enable_physics()` (exactly `cut_interaction`).

use bevy::prelude::*;
use bevy_rapier3d::prelude::{
    Collider as RapierCollider, CollisionGroups, NoUserData, RapierPhysicsPlugin,
    RigidBody as RapierRigidBody, RigidBodyDisabled,
};
use physics::{ColliderGroup, PhysicsEntityCommands};

fn app() -> App {
    use bevy_rapier3d::plugin::TimestepMode;
    use std::time::Duration;
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        RapierPhysicsPlugin::<NoUserData>::default(),
    ));
    // Advance time a fixed amount each `update()` (a tight test loop has ~0 wall-clock
    // delta, so physics would otherwise never step) and pin the timestep to match.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_secs_f64(1.0 / 60.0),
    ));
    app.insert_resource(TimestepMode::Fixed {
        dt: 1.0 / 60.0,
        substeps: 1,
    });
    app
}

fn spawn_limb(world: &mut World, parent: Entity, translation: Vec3) -> Entity {
    let limb = world
        .spawn((
            Transform::from_translation(translation),
            RapierRigidBody::Dynamic,
            RapierCollider::cuboid(0.15, 0.15, 0.15),
            CollisionGroups::from(ColliderGroup::Default),
        ))
        .id();
    world.entity_mut(parent).add_child(limb);
    limb
}

#[test]
fn cutting_a_body_into_limbs_does_not_panic() {
    let mut app = app();
    let world = app.world_mut();

    // Static ground.
    world.spawn((
        Transform::from_xyz(0.0, -1.0, 0.0),
        RapierRigidBody::Fixed,
        RapierCollider::cuboid(10.0, 0.5, 10.0),
        CollisionGroups::from(ColliderGroup::Default),
    ));

    // Creature root: dynamic body, collider on a child (like `player.bsn`).
    let root = world
        .spawn((Transform::from_xyz(0.0, 1.0, 0.0), RapierRigidBody::Dynamic))
        .id();
    // Character capsule collider (no rigid body of its own -> attaches to root).
    let capsule = world
        .spawn((
            Transform::from_xyz(0.0, 0.65, 0.0),
            RapierCollider::capsule_y(0.5, 0.18),
            CollisionGroups::from(ColliderGroup::CharacterColliders),
        ))
        .id();
    world.entity_mut(root).add_child(capsule);

    // Nested, mutually-overlapping limbs so contacts are generated.
    let torso = spawn_limb(world, root, Vec3::new(0.0, 0.0, 0.0));
    let mut limbs = vec![torso];
    limbs.push(spawn_limb(world, torso, Vec3::new(0.0, 0.4, 0.0))); // head
    limbs.push(spawn_limb(world, torso, Vec3::new(-0.2, 0.1, 0.0))); // arm_l
    limbs.push(spawn_limb(world, torso, Vec3::new(0.2, 0.1, 0.0))); // arm_r
    limbs.push(spawn_limb(world, torso, Vec3::new(-0.1, -0.4, 0.0))); // leg_l
    limbs.push(spawn_limb(world, torso, Vec3::new(0.1, -0.4, 0.0))); // leg_r

    // Let everything register and settle for a few steps.
    for _ in 0..4 {
        app.update();
    }

    // process_new_limbs: freeze every limb as an attached, inert body.
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .freeze(Some(ColliderGroup::AttachedLimbs));
    }
    app.world_mut().flush();
    for _ in 0..4 {
        app.update();
    }

    // cut_interaction: disable the root, then detach + enable every limb.
    app.world_mut().commands().entity(root).disable_physics();
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .remove::<ChildOf>()
            .enable_physics();
    }
    app.world_mut().flush();

    // Step: narrow phase runs here and (currently) trips the island assertion.
    for _ in 0..8 {
        app.update();
    }
}

/// Faithful variant: limb colliders are created *after* `freeze`, mirroring the
/// game where `physics::add_colliders` converts scene markers on a later frame
/// than `process_new_limbs`. And the cut double-processes each limb like the game
/// (`cut_interaction`'s inline `enable_physics` + `process_limb_removal`'s
/// `unfreeze(Default)`).
#[test]
fn cutting_with_deferred_colliders_does_not_panic() {
    let mut app = app();
    let world = app.world_mut();

    world.spawn((
        Transform::from_xyz(0.0, -1.0, 0.0),
        RapierRigidBody::Fixed,
        RapierCollider::cuboid(10.0, 0.5, 10.0),
        CollisionGroups::from(ColliderGroup::Default),
    ));

    let root = world
        .spawn((Transform::from_xyz(0.0, 1.0, 0.0), RapierRigidBody::Dynamic))
        .id();
    let capsule = world
        .spawn((
            Transform::from_xyz(0.0, 0.65, 0.0),
            RapierCollider::capsule_y(0.5, 0.18),
            CollisionGroups::from(ColliderGroup::CharacterColliders),
        ))
        .id();
    world.entity_mut(root).add_child(capsule);

    // Spawn limbs with a rigid body but NO collider yet.
    let spawn_bodyless = |world: &mut World, parent: Entity, t: Vec3| -> Entity {
        let e = world
            .spawn((Transform::from_translation(t), RapierRigidBody::Dynamic))
            .id();
        world.entity_mut(parent).add_child(e);
        e
    };
    let torso = spawn_bodyless(world, root, Vec3::new(0.0, 0.0, 0.0));
    let mut limbs = vec![torso];
    limbs.push(spawn_bodyless(world, torso, Vec3::new(0.0, 0.4, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(-0.2, 0.1, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(0.2, 0.1, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(-0.1, -0.4, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(0.1, -0.4, 0.0)));

    for _ in 0..4 {
        app.update();
    }

    // process_new_limbs: freeze BEFORE the colliders exist.
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .freeze(Some(ColliderGroup::AttachedLimbs));
    }
    app.world_mut().flush();
    for _ in 0..4 {
        app.update();
    }

    // add_colliders (later frame): create the colliders on now-disabled bodies.
    // The `AttachedLimbs` CollisionGroups was already placed on the entity by `freeze`.
    for &limb in &limbs {
        app.world_mut()
            .entity_mut(limb)
            .insert(RapierCollider::cuboid(0.15, 0.15, 0.15));
    }
    app.world_mut().flush();
    for _ in 0..4 {
        app.update();
    }

    // cut_interaction: disable root, detach + enable_physics each limb...
    app.world_mut().commands().entity(root).disable_physics();
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .remove::<ChildOf>()
            .enable_physics();
    }
    app.world_mut().flush();
    // ...then process_limb_removal drains limbs_to_remove -> unfreeze(Default).
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .unfreeze(Some(ColliderGroup::Default));
    }
    app.world_mut().flush();

    for _ in 0..8 {
        app.update();
    }
}

/// Isolates the actual invariant violation from the backtrace: a collider that is
/// created/attached AFTER its rigid body was disabled never inherits rapier's
/// `DisabledByParent` state, so it stays enabled on an islandless body. Put it in a
/// world-colliding group and a starting contact trips the island assertion.
#[test]
fn enabled_collider_on_disabled_body_panics() {
    let mut app = app();
    let world = app.world_mut();

    world.spawn((
        Transform::from_xyz(0.0, -1.0, 0.0),
        RapierRigidBody::Fixed,
        RapierCollider::cuboid(10.0, 0.5, 10.0),
        CollisionGroups::from(ColliderGroup::Default),
    ));

    // A dynamic body with NO collider yet.
    let body = world
        .spawn((Transform::from_xyz(0.0, 0.2, 0.0), RapierRigidBody::Dynamic))
        .id();
    for _ in 0..3 {
        app.update();
    }

    // Disable the body BEFORE it has a collider (mirrors freeze-before-add_colliders).
    app.world_mut().entity_mut(body).insert(RigidBodyDisabled);
    for _ in 0..3 {
        app.update();
    }

    // Now attach a collider in a world-colliding group. rapier will not disable it,
    // because the body's enabled->disabled change was already consumed.
    app.world_mut().entity_mut(body).insert((
        RapierCollider::cuboid(0.3, 0.3, 0.3),
        CollisionGroups::from(ColliderGroup::Default),
    ));

    for _ in 0..8 {
        app.update();
    }
}

/// Mirrors the game's two-frame cut timing with a physics STEP between each stage:
///   frame A: `cut_interaction` -> disable_physics(root) + per-limb enable_physics; step
///   frame B: `process_limb_removal` -> per-limb unfreeze(Default);               step
/// so the intermediate transitional states are actually simulated.
#[test]
fn cut_two_frame_timing_does_not_panic() {
    let mut app = app();
    let world = app.world_mut();

    world.spawn((
        Transform::from_xyz(0.0, -1.0, 0.0),
        RapierRigidBody::Fixed,
        RapierCollider::cuboid(10.0, 0.5, 10.0),
        CollisionGroups::from(ColliderGroup::Default),
    ));

    let root = world
        .spawn((Transform::from_xyz(0.0, 1.0, 0.0), RapierRigidBody::Dynamic))
        .id();
    let capsule = world
        .spawn((
            Transform::from_xyz(0.0, 0.65, 0.0),
            RapierCollider::capsule_y(0.5, 0.18),
            CollisionGroups::from(ColliderGroup::CharacterColliders),
        ))
        .id();
    world.entity_mut(root).add_child(capsule);

    let spawn_bodyless = |world: &mut World, parent: Entity, t: Vec3| -> Entity {
        let e = world
            .spawn((Transform::from_translation(t), RapierRigidBody::Dynamic))
            .id();
        world.entity_mut(parent).add_child(e);
        e
    };
    let torso = spawn_bodyless(world, root, Vec3::new(0.0, 0.0, 0.0));
    let mut limbs = vec![torso];
    limbs.push(spawn_bodyless(world, torso, Vec3::new(0.0, 0.4, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(-0.2, 0.1, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(0.2, 0.1, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(-0.1, -0.4, 0.0)));
    limbs.push(spawn_bodyless(world, torso, Vec3::new(0.1, -0.4, 0.0)));

    for _ in 0..4 {
        app.update();
    }
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .freeze(Some(ColliderGroup::AttachedLimbs));
    }
    app.world_mut().flush();
    for _ in 0..3 {
        app.update();
    }
    for &limb in &limbs {
        app.world_mut()
            .entity_mut(limb)
            .insert(RapierCollider::cuboid(0.15, 0.15, 0.15));
    }
    app.world_mut().flush();
    for _ in 0..3 {
        app.update();
    }

    // Frame A: cut_interaction.
    app.world_mut().commands().entity(root).disable_physics();
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .remove::<ChildOf>()
            .enable_physics();
    }
    app.update(); // <-- physics step on the intermediate state

    // Frame B: process_limb_removal.
    for &limb in &limbs {
        app.world_mut()
            .commands()
            .entity(limb)
            .unfreeze(Some(ColliderGroup::Default));
    }
    app.update();

    for _ in 0..8 {
        app.update();
    }
}

/// Does a kinematic (frozen) child limb track its moving dynamic parent, or drift/scatter?
#[test]
fn frozen_limb_tracks_moving_parent() {
    let mut app = app();
    let world = app.world_mut();
    let root = world
        .spawn((
            Transform::from_xyz(0.0, 5.0, 0.0),
            RapierRigidBody::Dynamic,
            RapierCollider::cuboid(0.2, 0.3, 0.2),
            CollisionGroups::from(ColliderGroup::CharacterColliders),
        ))
        .id();
    let limb = spawn_limb(world, root, Vec3::new(0.0, 0.4, 0.0));
    // Freeze the limb like process_new_limbs does.
    app.world_mut()
        .commands()
        .entity(limb)
        .freeze(Some(ColliderGroup::AttachedLimbs));
    app.world_mut().flush();

    for i in 0..30 {
        app.update();
        if i % 6 == 0 {
            let rg = app
                .world()
                .entity(root)
                .get::<GlobalTransform>()
                .unwrap()
                .translation();
            let lg = app
                .world()
                .entity(limb)
                .get::<GlobalTransform>()
                .unwrap()
                .translation();
            println!(
                "frame {i}: root_y={:.3} limb_y={:.3} offset={:.3}",
                rg.y,
                lg.y,
                lg.y - rg.y
            );
        }
    }
}

/// Do overlapping dynamic limbs (Default group) explode during the pre-freeze window?
#[test]
fn overlapping_dynamic_limbs_before_freeze() {
    let mut app = app();
    let world = app.world_mut();
    world.spawn((
        Transform::from_xyz(0.0, -1.0, 0.0),
        RapierRigidBody::Fixed,
        RapierCollider::cuboid(10.0, 0.5, 10.0),
        CollisionGroups::from(ColliderGroup::Default),
    ));
    let root = world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            RapierRigidBody::Dynamic,
            RapierCollider::cuboid(0.2, 0.3, 0.2),
            CollisionGroups::from(ColliderGroup::CharacterColliders),
        ))
        .id();
    // Overlapping, nested like the human rig — all Default group, NOT frozen.
    let torso = spawn_limb(world, root, Vec3::new(0.0, 0.0, 0.0));
    let parts = [
        (0.0, 0.4, 0.0),
        (-0.2, 0.1, 0.0),
        (0.2, 0.1, 0.0),
        (-0.1, -0.4, 0.0),
        (0.1, -0.4, 0.0),
    ];
    for p in parts {
        spawn_limb(world, torso, Vec3::new(p.0, p.1, p.2));
    }
    for i in 0..20 {
        app.update();
        let lg = app
            .world()
            .entity(torso)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        if i % 5 == 0 || !lg.is_finite() {
            println!("frame {i}: torso_global={lg:?} finite={}", lg.is_finite());
        }
    }
}

/// Confirm the parry BVH crash mechanism: several colliders at the EXACT same position.
#[test]
fn coincident_colliders_bvh() {
    let mut app = app();
    for _ in 0..6 {
        app.world_mut().spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            RapierRigidBody::Dynamic,
            RapierCollider::cuboid(0.15, 0.15, 0.15),
            CollisionGroups::from(ColliderGroup::Default),
        ));
    }
    for _ in 0..5 {
        app.update();
    }
    println!("COINCIDENT_OK");
}

/// And a NaN-position collider.
#[test]
fn nan_position_collider_bvh() {
    let mut app = app();
    app.world_mut().spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        RapierRigidBody::Fixed,
        RapierCollider::cuboid(1.0, 1.0, 1.0),
        CollisionGroups::from(ColliderGroup::Default),
    ));
    app.world_mut().spawn((
        Transform::from_xyz(f32::NAN, 0.0, 0.0),
        RapierRigidBody::KinematicPositionBased,
        RapierCollider::cuboid(0.15, 0.15, 0.15),
        CollisionGroups::from(ColliderGroup::Default),
    ));
    for _ in 0..5 {
        app.update();
    }
    println!("NAN_OK");
}

/// Nested chain of frozen (kinematic) limbs off a MOVING + ROTATING dynamic root,
/// mirroring the human rig (torso->head, torso->arm->hand, torso->leg->foot).
/// Checks whether limbs keep their attachment offset or diverge/explode.
#[test]
fn nested_frozen_limbs_track_moving_parent() {
    use bevy_rapier3d::prelude::Velocity;
    let mut app = app();
    let world = app.world_mut();

    world.spawn((
        Transform::from_xyz(0.0, -1.0, 0.0),
        RapierRigidBody::Fixed,
        RapierCollider::cuboid(50.0, 0.5, 50.0),
        CollisionGroups::from(ColliderGroup::Default),
    ));
    let root = world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            RapierRigidBody::Dynamic,
            RapierCollider::capsule_y(0.5, 0.18),
            CollisionGroups::from(ColliderGroup::CharacterColliders),
            Velocity::default(),
        ))
        .id();
    let torso = spawn_limb(world, root, Vec3::new(0.0, 0.0, 0.0));
    let head = spawn_limb(world, torso, Vec3::new(0.0, 0.4, 0.0));
    let arm = spawn_limb(world, torso, Vec3::new(0.2, 0.1, 0.0));
    let hand = spawn_limb(world, arm, Vec3::new(0.25, 0.0, 0.0));
    let leg = spawn_limb(world, torso, Vec3::new(0.1, -0.4, 0.0));
    let foot = spawn_limb(world, leg, Vec3::new(0.0, -0.3, 0.0));
    let limbs = [
        ("torso", torso),
        ("head", head),
        ("arm", arm),
        ("hand", hand),
        ("leg", leg),
        ("foot", foot),
    ];
    for (_, l) in limbs {
        app.world_mut()
            .commands()
            .entity(l)
            .freeze(Some(ColliderGroup::AttachedLimbs));
    }
    app.world_mut().flush();
    for _ in 0..5 {
        app.update();
    }

    // Baseline offsets (limb world - root world).
    let root_p0 = app
        .world()
        .entity(root)
        .get::<GlobalTransform>()
        .unwrap()
        .translation();
    let base: Vec<_> = limbs
        .iter()
        .map(|(n, l)| {
            (
                *n,
                app.world()
                    .entity(*l)
                    .get::<GlobalTransform>()
                    .unwrap()
                    .translation()
                    - root_p0,
            )
        })
        .collect();

    // Drive the root: move AND spin (yaw), like the character walking + turning.
    for i in 0..90 {
        {
            let mut root_mut = app.world_mut().entity_mut(root);
            let mut v = root_mut.get_mut::<Velocity>().unwrap();
            v.linear = Vec3::new(3.0, 0.0, 2.0);
            v.angular = Vec3::new(0.0, 4.0, 0.0);
        }
        app.update();
        if i % 15 == 0 || i == 89 {
            let rp = app
                .world()
                .entity(root)
                .get::<GlobalTransform>()
                .unwrap()
                .translation();
            let worst = limbs
                .iter()
                .zip(&base)
                .map(|((n, l), (_, b0))| {
                    let off = app
                        .world()
                        .entity(*l)
                        .get::<GlobalTransform>()
                        .unwrap()
                        .translation()
                        - rp;
                    (*n, (off.length() - b0.length()).abs(), off.is_finite())
                })
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap();
            println!(
                "frame {i}: root=({:.1},{:.1},{:.1}) worst_limb={} offset_drift={:.4} finite={}",
                rp.x, rp.y, rp.z, worst.0, worst.1, worst.2
            );
        }
    }
}
