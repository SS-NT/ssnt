use bevy::{prelude::*, window::PrimaryWindow};

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
        app.add_plugins((SplashPlugin, MainMenuPlugin, PauseMenuPlugin, LobbyPlugin));
    }
}

/// Run criteria that returns true if the primary window exists.
pub fn has_window(query: Query<(), With<PrimaryWindow>>) -> bool {
    !query.is_empty()
}
