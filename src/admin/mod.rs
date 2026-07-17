use bevy::prelude::{App, Plugin};

pub(crate) mod inspector;
mod map;
mod spawning;

pub(crate) struct AdminPlugin;

impl Plugin for AdminPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            spawning::SpawningPlugin,
            map::MapManagementPlugin,
            inspector::InspectorPlugin,
        ));
    }
}
