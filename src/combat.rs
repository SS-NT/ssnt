use bevy::{ecs::system::SystemParam, prelude::*, reflect::TypeUuid, window::PrimaryWindow};
use bevy_egui::{egui, EguiContexts};
use networking::{
    component::AppExt,
    is_server,
    messaging::{AppExt as MessageExt, MessageEvent, MessageSender},
    spawning::{ClientControlled, ClientControls},
    variable::{NetworkVar, ServerVar},
    Networked, Players,
};
use serde::{Deserialize, Serialize};

use crate::{
    body::{Hand, Hands},
    camera::MainCamera,
    items::containers::Container,
    ui::has_window,
};

use self::ranged::RangedPlugin;

pub mod damage;
mod ranged;
pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<UpdateCombatModeRequest>()
            .add_network_message::<CombatInput>()
            .add_networked_component::<CombatMode, CombatModeClient>();
        if is_server(app) {
            app.add_event::<CombatInputEvent>()
                .add_systems(Update, (receive_combat_mode_request, handle_attack_request));
        } else {
            app.add_systems(
                Update,
                (
                    client_toggle_combat_mode,
                    (
                        (client_calculate_aim, client_combat_input).chain(),
                        client_combat_mode_ui.run_if(has_window),
                    ),
                )
                    .chain(),
            );
        }
        app.add_plugins(RangedPlugin);
    }
}

#[derive(Default, Component, Networked)]
#[networked(client = "CombatModeClient")]
pub struct CombatMode {
    enabled: NetworkVar<bool>,
}

impl CombatMode {
    pub fn set(&mut self, enabled: bool) {
        *self.enabled = enabled;
    }
}

#[derive(Component, Networked, TypeUuid, Default)]
#[networked(server = "CombatMode")]
#[uuid = "bfe1d314-6e1a-4e9d-b871-d8e9879e27ea"]
pub struct CombatModeClient {
    enabled: ServerVar<bool>,
    pub aim: Aim,
}

#[derive(SystemParam)]
pub struct ClientCombatModeStatus<'w, 's> {
    controlled: Query<'w, 's, &'static CombatModeClient, With<ClientControlled>>,
}

impl<'w, 's> ClientCombatModeStatus<'w, 's> {
    pub fn is_enabled(&self) -> bool {
        self.controlled
            .get_single()
            .map(|mode| *mode.enabled)
            .unwrap_or(false)
    }
}

#[derive(Serialize, Deserialize)]
struct UpdateCombatModeRequest {
    enabled: bool,
}

fn receive_combat_mode_request(
    mut messages: EventReader<MessageEvent<UpdateCombatModeRequest>>,
    players: Res<Players>,
    controlled: Res<ClientControls>,
    mut modes: Query<&mut CombatMode>,
    mut commands: Commands,
) {
    for event in messages.iter() {
        let Some(player) = players.get(event.connection) else {
            continue;
        };
        let Some(entity) = controlled.controlled_entity(player.id) else {
            continue;
        };
        if let Ok(mut mode) = modes.get_mut(entity) {
            mode.set(event.message.enabled);
        } else if event.message.enabled {
            commands.entity(entity).insert(CombatMode {
                enabled: true.into(),
            });
        }
    }
}

fn client_combat_mode_ui(mut contexts: EguiContexts, status: ClientCombatModeStatus) {
    // Show UI only if combat mode is enabled
    if !status.is_enabled() {
        return;
    }
    egui::Area::new("combat_mode_indicator")
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 0.0))
        .show(contexts.ctx_mut(), |ui| {
            ui.vertical_centered_justified(|ui| {
                ui.label(
                    egui::RichText::new("COMBAT MODE")
                        .color(egui::Rgba::RED)
                        .size(21.0),
                );
            });
        });
}

fn client_toggle_combat_mode(
    keys: Res<Input<KeyCode>>,
    status: ClientCombatModeStatus,
    mut sender: MessageSender,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }

    let new_enabled = !status.is_enabled();

    sender.send_to_server(&UpdateCombatModeRequest {
        enabled: new_enabled,
    });
}

#[derive(Default, Clone, Copy, Serialize, Deserialize)]
pub struct Aim {
    pub target_position: Vec3,
    // TODO: Don't allow client to send this
    pub origin: Vec3,
}

/// At what height ranged weapons are aimed.
// TODO: Replace with height depending on character
const RANGED_AIM_HEIGHT: f32 = 0.85;

fn client_calculate_aim(
    mut players: Query<(&mut CombatModeClient, &GlobalTransform), With<ClientControlled>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    if players.is_empty() {
        return;
    }

    let Ok(window) = windows.get_single() else {
        return;
    };
    let Some((camera, camera_transform)) = cameras.iter().next() else {
        return;
    };
    let Some(cursor_position) = window.cursor_position() else {
        return;
    };

    let Some(ray) = camera.viewport_to_world(camera_transform, cursor_position) else {
        return;
    };

    let Some(toi) = ray.intersect_plane(Vec3::new(0.0, RANGED_AIM_HEIGHT, 0.0), Vec3::Y) else {
        return;
    };
    let target_position = ray.origin + ray.direction * toi;

    for (mut combat, transform) in players.iter_mut() {
        combat.aim = Aim {
            origin: transform.translation(),
            target_position,
        };
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct CombatInput {
    aim: Aim,
    primary_attack: bool,
}

fn client_combat_input(
    combat_mode: ClientCombatModeStatus,
    buttons: Res<Input<MouseButton>>,
    players: Query<&CombatModeClient, With<ClientControlled>>,
    mut sender: MessageSender,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }

    if !combat_mode.is_enabled() {
        return;
    }

    let combat = players.single();

    // TODO: Should be unreliable and buffered, including prediction
    sender.send_to_server(&CombatInput {
        aim: combat.aim,
        primary_attack: true,
    });
}

#[derive(Event)]
struct CombatInputEvent {
    #[allow(dead_code)]
    actor: Entity,
    input: CombatInput,
    wielded_weapon: Option<Entity>,
    #[allow(dead_code)]
    used_hand: Option<Entity>,
}

fn handle_attack_request(
    mut events: EventReader<MessageEvent<CombatInput>>,
    players: Res<Players>,
    controls: Res<ClientControls>,
    bodies: Query<&Hands>,
    hand_query: Query<(Entity, &Container), With<Hand>>,
    mut attack_event: EventWriter<CombatInputEvent>,
) {
    for event in events.iter() {
        let Some(player) = players.get(event.connection).map(|p| p.id) else {
            continue;
        };
        let Some(player_entity) = controls.controlled_entity(player) else {
            continue;
        };

        let hand = bodies
            .get(player_entity)
            .ok()
            .and_then(|hands| hand_query.get(hands.active_hand()).ok());
        let wielded_weapon =
            hand.and_then(|(_, container)| container.iter().next().map(|(_, item)| *item));
        let used_hand = hand.unzip().0;

        attack_event.send(CombatInputEvent {
            actor: player_entity,
            input: event.message,
            wielded_weapon,
            used_hand,
        });
    }
}
