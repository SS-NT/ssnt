use bevy::{prelude::*, window::PrimaryWindow};
use bevy_egui::EguiContexts;
use networking::{
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent},
    spawning::ClientControls,
    visibility::AlwaysVisible,
    Players,
};
use serde::{Deserialize, Serialize};

use self::{
    lobby::LobbyPlugin, main_menu::MainMenuPlugin, pause_menu::PauseMenuPlugin,
    splash::SplashPlugin,
};

mod lobby;
mod main_menu;
mod pause_menu;
mod splash;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<CloseUiMessage>();
        if is_server(app) {
            app.add_systems(Update, (handle_close_ui, close_unused_uis));
        } else {
            app.add_plugins((SplashPlugin, MainMenuPlugin, PauseMenuPlugin, LobbyPlugin))
                .add_systems(
                    PreUpdate,
                    (absorb_egui_inputs,)
                        .after(bevy_egui::systems::process_input_system)
                        .before(bevy_egui::EguiSet::BeginFrame),
                );
        }
    }
}

/// Run criteria that returns true if the primary window exists.
pub fn has_window(query: Query<(), With<PrimaryWindow>>) -> bool {
    !query.is_empty()
}

/// Prevents bevy systems from receiving input when it's used by the UI
fn absorb_egui_inputs(
    mut mouse: ResMut<Input<MouseButton>>,
    mut keyboard: ResMut<Input<KeyCode>>,
    mut contexts: EguiContexts,
) {
    if contexts.ctx_mut().is_pointer_over_area() {
        mouse.reset_all();
    }

    if contexts.ctx_mut().wants_keyboard_input() {
        keyboard.reset_all();
    }
}

/// Marks an entity as a networked UI
#[derive(Component)]
pub struct NetworkUi;

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct CloseUiMessage {
    pub ui: NetworkIdentity,
}

fn handle_close_ui(
    mut messages: EventReader<MessageEvent<CloseUiMessage>>,
    mut uis: Query<&mut AlwaysVisible, With<NetworkUi>>,
    players: Res<Players>,
    controls: Res<ClientControls>,
    identities: Res<NetworkIdentities>,
) {
    for event in messages.iter() {
        let Some(player) = players.get(event.connection) else {
            continue;
        };
        let Some(player_entity) = controls.controlled_entity(player.id) else {
            continue;
        };

        let Some(ui_entity) = identities.get_entity(event.message.ui) else {
            continue;
        };

        let Ok(mut visible) = uis.get_mut(ui_entity) else {
            continue;
        };

        if let Some(index) = visible.0.iter().position(|&e| e == player_entity) {
            visible.0.swap_remove(index);
        }
    }
}

fn close_unused_uis(uis: Query<(Entity, &AlwaysVisible), With<NetworkUi>>, mut commands: Commands) {
    for (ui_entity, visible) in uis.iter() {
        if visible.0.is_empty() {
            commands.entity(ui_entity).despawn();
        }
    }
}
