use bevy::{prelude::*, window::PrimaryWindow};
use bevy_egui::EguiContexts;

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
        app.add_plugins((SplashPlugin, MainMenuPlugin, PauseMenuPlugin, LobbyPlugin))
            .add_systems(
                PreUpdate,
                (absorb_egui_inputs,)
                    .after(bevy_egui::systems::process_input_system)
                    .before(bevy_egui::EguiSet::BeginFrame),
            );
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
