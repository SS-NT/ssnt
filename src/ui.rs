use bevy::prelude::*;

use self::{lobby::LobbyPlugin, main_menu::MainMenuPlugin, splash::SplashPlugin};

mod lobby;
mod main_menu;
mod splash;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(SplashPlugin)
            .add_plugin(MainMenuPlugin)
            .add_plugin(LobbyPlugin);
    }
}
