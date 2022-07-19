use bevy::prelude::{App, Plugin};

use self::spawning::SpawningPlugin;

mod spawning;

pub(crate) struct AdminPlugin;

impl Plugin for AdminPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(SpawningPlugin);
    }
}
