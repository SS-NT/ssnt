use bevy::prelude::{App, Plugin};

mod map;
mod spawning;

pub(crate) struct AdminPlugin;

impl Plugin for AdminPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(spawning::SpawningPlugin)
            .add_plugin(map::MapManagementPlugin);
    }
}
