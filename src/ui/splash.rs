use bevy::prelude::*;

use crate::{components::despawn_with, GameState};

pub struct SplashPlugin;

impl Plugin for SplashPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Splash), splash_setup)
            .add_systems(Update, countdown.run_if(in_state(GameState::Splash)))
            .add_systems(OnExit(GameState::Splash), despawn_with::<OnSplashScreen>);
    }
}

#[derive(Component)]
struct OnSplashScreen;

// Newtype to use a `Timer` for this screen as a resource
#[derive(Deref, DerefMut, Resource)]
struct SplashTimer(Timer);

#[derive(Deref, DerefMut, Resource)]
struct SplashFadeElement(Entity);

fn splash_setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    let background = asset_server.load("artwork/ProbablyNot_Spaced.png");
    let logo = asset_server.load("artwork/logo.png");
    let mut fade = None;
    // Display the logo
    commands
        .spawn((
            ImageBundle {
                style: Style {
                    // This will center the logo
                    margin: UiRect::all(Val::Auto),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                image: UiImage::new(background),
                ..default()
            },
            OnSplashScreen,
        ))
        .with_children(|parent| {
            parent.spawn(ImageBundle {
                style: Style {
                    margin: UiRect::all(Val::Auto),
                    ..default()
                },
                image: UiImage::new(logo),
                ..default()
            });
            fade = Some(
                parent
                    .spawn(NodeBundle {
                        style: Style {
                            position_type: PositionType::Absolute,
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        background_color: Color::rgba(0.0, 0.0, 0.0, 0.0).into(),
                        ..default()
                    })
                    .id(),
            );
        });
    commands.insert_resource(SplashFadeElement(fade.unwrap()));
    // Insert the timer as a resource
    commands.insert_resource(SplashTimer(Timer::from_seconds(3.0, TimerMode::Once)));
}

// Tick the timer, and change state when finished
fn countdown(
    mut game_state: ResMut<NextState<GameState>>,
    time: Res<Time>,
    mut timer: ResMut<SplashTimer>,
    fade_element: Res<SplashFadeElement>,
    mut colors: Query<&mut BackgroundColor>,
) {
    const FADE_FROM_TIME: f32 = 0.7;
    const FADE_TO_TIME: f32 = 0.90;

    if timer.tick(time.delta()).finished() {
        game_state.set(GameState::MainMenu);
    }

    // Fade out
    let mut color = colors.get_mut(**fade_element).unwrap();
    let alpha = map_range((FADE_FROM_TIME, FADE_TO_TIME), (0.0, 1.0), timer.percent());
    color.0.set_a(alpha);
}

fn map_range(from_range: (f32, f32), to_range: (f32, f32), s: f32) -> f32 {
    to_range.0 + (s - from_range.0) * (to_range.1 - to_range.0) / (from_range.1 - from_range.0)
}
