use std::{borrow::Cow, ops::Range};

use bevy::{
    asset::{AssetPathId, HandleId},
    prelude::*,
    utils::HashMap,
};
use bevy_egui::{egui, EguiContexts};
use networking::{
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    spawning::ClientControls,
    Players,
};
use serde::{Deserialize, Serialize};
use speech::AccentDefinition;

use crate::{camera::MainCamera, ui::has_window, GameState};

pub struct CommunicationPlugin;

impl Plugin for CommunicationPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<SpeakMessage>()
            .add_network_message::<SpeechMessage>();

        if is_server(app) {
            app.add_systems(Update, handle_speech);
        } else {
            app.init_resource::<ClientChat>().add_systems(
                Update,
                (
                    (client_chat_box, client_speech_bubbles)
                        .run_if(has_window)
                        .run_if(in_state(GameState::Game)),
                    client_handle_chat,
                ),
            );
        }
    }
}

#[derive(Serialize, Deserialize)]
struct RadioChannel(pub u32);

#[derive(Serialize, Deserialize)]
enum ChatKind {
    Local,
    Ooc,
    Radio(RadioChannel),
}

/// A chat message in serializable form.
#[derive(Serialize, Deserialize, Default)]
struct ChatMessage {
    text: String,
    sections: Vec<ChatSection>,
    /// The part of the text that is audible
    spoken_range: Option<Range<usize>>,
}

#[derive(Serialize, Deserialize)]
struct ChatSection {
    /// Part of the chat message this format applies to
    range: Range<usize>,
    format: ChatFormat,
}

#[derive(Serialize, Deserialize, Default, Clone, Copy)]
struct ChatFormat {
    italics: bool,
    underline: bool,
    // TODO: Support bold text
    bold: bool,
}

impl From<ChatFormat> for egui::TextFormat {
    fn from(value: ChatFormat) -> Self {
        egui::TextFormat {
            italics: value.italics,
            underline: value
                .underline
                .then_some(egui::Stroke {
                    color: egui::Color32::WHITE,
                    width: 1.0,
                })
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl ChatMessage {
    fn section(&mut self, text: &str, format: ChatFormat) {
        let start = self.text.len();
        self.text += text;
        self.sections.push(ChatSection {
            range: start..self.text.len(),
            format,
        });
    }

    /// Append text without creating a new section
    fn append(&mut self, text: &str) {
        self.text += text;
        match self.sections.last_mut() {
            Some(s) => {
                // Extend the last section
                s.range.end = self.text.len();
            }
            None => {
                // No section yet, create a default one
                self.sections.push(ChatSection {
                    range: 0..self.text.len(),
                    format: Default::default(),
                });
            }
        }
    }

    fn append_speech(&mut self, text: &str) {
        let start = self.text.len();
        self.append(text);
        self.spoken_range = Some(start..self.text.len());
    }

    fn append_to(&self, layout: &mut egui::text::LayoutJob) {
        Self::add_newline(layout);

        let base_index = layout.text.len();
        layout.text += self.text.as_str();

        for section in &self.sections {
            layout.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: (base_index + section.range.start)..(base_index + section.range.end),
                format: section.format.into(),
            });
        }
    }

    fn append_spoken_part(&self, layout: &mut egui::text::LayoutJob) -> Option<()> {
        let range = self.spoken_range.clone()?;
        let spoken = &self.text[range.clone()];

        Self::add_newline(layout);
        let base_index = layout.text.len();
        layout.text += spoken;

        // Apply the styling
        // We need to loop all sections because the spoken text can be composed of different sections
        let mut range_index = range.start;
        for section in &self.sections {
            if section.range.contains(&range_index) {
                let end = section.range.end.min(range.end);
                layout.sections.push(egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: (base_index + range_index - range.start)
                        ..(base_index + end - range.start),
                    format: section.format.into(),
                });
                range_index = end;
                if range_index >= range.end {
                    break;
                }
            }
        }

        Some(())
    }

    fn add_newline(layout: &mut egui::text::LayoutJob) {
        if !layout.sections.is_empty() {
            layout.append("\n", 0.0, egui::TextFormat::default());
        }
    }
}

/// What's the name that appears when the entity speaks.
#[derive(Component)]
pub struct SpeechName(pub String);

/// Client message to say something
#[derive(Serialize, Deserialize)]
struct SpeakMessage {
    text: String,
    kind: ChatKind,
    // these are manually selected by user for now
    selected_accents: Vec<(AssetPathId, u64)>,
}

/// Server message when someone said something
#[derive(Serialize, Deserialize)]
struct SpeechMessage {
    message: ChatMessage,
    speaker: Option<NetworkIdentity>,
}

fn handle_speech(
    mut messages: EventReader<MessageEvent<SpeakMessage>>,
    players: Res<Players>,
    controlled: Res<ClientControls>,
    identities: Res<NetworkIdentities>,
    names: Query<AnyOf<(&SpeechName, &Name)>>,
    accent_data: Res<Assets<AccentDefinition>>,
    mut sender: MessageSender,
) {
    for event in messages.iter() {
        let Some(player) = players.get(event.connection) else {
            continue;
        };

        let Some(player_entity) = controlled.controlled_entity(player.id) else {
            continue;
        };

        // Get name for speaker
        let name = match names.get(player_entity) {
            Ok((Some(speech_name), _)) => speech_name.0.clone(),
            Ok((_, Some(name))) => name.as_str().to_owned(),
            _ => "Unknown".to_owned(),
        };

        let text = event
            .message
            .selected_accents
            .iter()
            .filter_map(|(handle_id, intensity)| {
                accent_data
                    .get(&accent_data.get_handle(*handle_id))
                    .map(|accent| (accent, intensity))
            })
            .fold(
                Cow::Borrowed(&event.message.text),
                |acc, (accent, intensity)| {
                    Cow::Owned(accent.body.say_it(&acc, *intensity).into_owned())
                },
            );
        let text = text.as_ref();

        // TODO: Use chat kind (ex. OOC)

        let mut message = ChatMessage::default();
        message.section(
            &name,
            ChatFormat {
                bold: true,
                ..Default::default()
            },
        );
        message.section(" says, \"", Default::default());
        message.append_speech(text);
        message.append("\"");

        info!(
            player = player.id.to_string().as_str(),
            text, "Chat message"
        );

        sender.send(
            &SpeechMessage {
                message,
                speaker: identities.get_identity(player_entity),
            },
            // TODO: Respect local chat, only send to nearby & hearing players
            MessageReceivers::AllPlayers,
        );
    }
}

#[derive(Resource, Default)]
struct ClientChat {
    input_chat: String,
    history: egui::text::LayoutJob,
    bubbles: HashMap<NetworkIdentity, SpeechBubble>,
    bubble_id: usize,
    // these are manually selected by user for now
    selected_accents: Vec<HandleId>,
}

struct SpeechBubble {
    id: usize,
    text: egui::text::LayoutJob,
    when: f32,
}

fn client_chat_box(
    mut contexts: EguiContexts,
    mut data: ResMut<ClientChat>,
    mut keyboard: ResMut<Input<KeyCode>>,
    mut sender: MessageSender,
    accent_data: Res<Assets<AccentDefinition>>,
) {
    egui::Window::new("Chat")
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::Vec2::ZERO)
        .default_size(egui::vec2(200.0, 800.0))
        .resizable(true)
        .show(contexts.ctx_mut(), |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // This is probably expensive?
                ui.label(data.history.clone());
            });

            // accent list
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                for (handle_id, accent) in accent_data.iter() {
                    let has_accent = data.selected_accents.contains(&handle_id);
                    let mut button_active = has_accent;

                    let mut response = ui.toggle_value(&mut button_active, &accent.name);
                    if response.clicked() {
                        if has_accent {
                            if let Some(index) =
                                data.selected_accents.iter().position(|i| *i == handle_id)
                            {
                                data.selected_accents.remove(index);
                            }
                        } else {
                            data.selected_accents.push(handle_id);
                        }
                        response.mark_changed();
                    }
                }
            });
            ui.separator();

            let response = egui::TextEdit::singleline(&mut data.input_chat)
                .hint_text("Talk")
                .id_source("chat_input")
                .show(ui)
                .response;

            // Focus chat if chat key is pressed
            if keyboard.clear_just_pressed(KeyCode::T) {
                response.request_focus();
            }
            if response.lost_focus()
                && response
                    .ctx
                    .input(|input| input.key_pressed(egui::Key::Enter))
            {
                if !data.input_chat.trim().is_empty() {
                    sender.send_to_server(&SpeakMessage {
                        text: std::mem::take(&mut data.input_chat),
                        kind: ChatKind::Local,
                        selected_accents: data
                            .selected_accents
                            .iter()
                            .map(|handle| match handle {
                                HandleId::Id(_, _) => panic!("Accent must be asset"),
                                // accent intensity is hardcoded to 0 for now
                                HandleId::AssetPathId(id) => (id.to_owned(), 0),
                            })
                            .collect(),
                    });
                }
                data.input_chat.clear();
            }
        });
}

fn client_handle_chat(
    mut messages: EventReader<MessageEvent<SpeechMessage>>,
    mut data: ResMut<ClientChat>,
    time: Res<Time>,
) {
    for event in messages.iter() {
        let data = &mut *data;
        event.message.message.append_to(&mut data.history);

        // Check if we should add a speech bubble
        let Some(speaker) = event.message.speaker else {
            continue;
        };

        if event.message.message.spoken_range.is_none() {
            continue;
        }

        let bubble = data
            .bubbles
            .entry(speaker)
            .and_modify(|bubble| bubble.when = time.elapsed_seconds())
            .or_insert_with(|| {
                let id = data.bubble_id.wrapping_add(1);
                data.bubble_id = id;
                SpeechBubble {
                    id,
                    text: Default::default(),
                    when: time.elapsed_seconds(),
                }
            });
        event.message.message.append_spoken_part(&mut bubble.text);
    }
}

// TODO: Duration depending on text length
// TODO: Add accessibility setting
const SPEECH_BUBBLE_DURATION: f32 = 4.0;

fn client_speech_bubbles(
    mut contexts: EguiContexts,
    mut data: ResMut<ClientChat>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    transforms: Query<&GlobalTransform>,
    identities: Res<NetworkIdentities>,
    time: Res<Time>,
) {
    let Ok((camera, camera_transform)) = camera.get_single() else {
        return;
    };

    data.bubbles.retain(|&speaker, bubble| {
        if bubble.when + SPEECH_BUBBLE_DURATION < time.elapsed_seconds() {
            return false;
        }

        let Some(entity) = identities.get_entity(speaker) else {
            return true;
        };

        let Ok(transform) = transforms.get(entity) else {
            return true;
        };

        // TODO: Calculate offset from character bounding box
        let offset = Vec3::Y * 1.8;

        let Some(screen_position) =
            camera.world_to_viewport(camera_transform, transform.translation() + offset)
        else {
            return true;
        };

        egui::Window::new("")
            .id(egui::Id::new("speech_bubble").with(bubble.id))
            .title_bar(false)
            .resizable(false)
            .fixed_pos(egui::pos2(screen_position.x, screen_position.y))
            .pivot(egui::Align2::CENTER_BOTTOM)
            .show(contexts.ctx_mut(), |ui| {
                // TODO: Use a non-allocating Galley instead
                ui.label(bubble.text.clone());
            });

        true
    });
}
