use std::{fs::read_to_string, time::Duration};

use async_compat::Compat;
use bevy::{
    prelude::{error, Res, Resource},
    tasks::IoTaskPool,
};
use serde::{Deserialize, Serialize};
use tokio::time::{interval, MissedTickBehavior};

use crate::{ArgCommands, Args};

#[derive(Default, Deserialize, Resource)]
pub struct ServerConfig {
    pub registration: Option<ServerRegistration>,
}

#[derive(Deserialize, Clone)]
pub struct ServerRegistration {
    api_url: String,
    pub private_key: [u8; 32],
}

const DEFAULT_SERVER_CONFIG_FILE: &str = "server-config.toml";

pub fn load_server_config() -> Result<ServerConfig, toml::de::Error> {
    let text = match read_to_string(DEFAULT_SERVER_CONFIG_FILE) {
        Ok(t) => t,
        Err(_) => return Ok(ServerConfig::default()),
    };
    toml::from_str(&text)
}

const SERVER_PING_MUTATION: &str = "mutation ping($privateKey: [Int!], $port: Int!) {
  serverPing(input: {privateKey: $privateKey, port: $port}) {
    id
  }
}";

#[derive(Serialize)]
struct ServerPingMutation {
    query: &'static str,
    variables: ServerPingMutationVariables,
}

impl ServerPingMutation {
    fn new(private_key: [u8; 32], port: u16) -> Self {
        Self {
            query: SERVER_PING_MUTATION,
            variables: ServerPingMutationVariables { private_key, port },
        }
    }
}

#[derive(Serialize)]
struct ServerPingMutationVariables {
    port: u16,
    #[serde(rename = "privateKey")]
    private_key: [u8; 32],
}

pub(crate) fn server_startup(config: Res<ServerConfig>, args: Res<Args>) {
    if let Some(registration) = config.registration.as_ref().cloned() {
        let client = reqwest::Client::new();
        let port = match args.command {
            Some(ArgCommands::Host { bind_address, .. }) => bind_address.port(),
            _ => panic!(),
        };

        // Ping central api server at an interval
        let ping_future = async move {
            let mut interval = interval(Duration::from_secs(20));
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

            loop {
                let result = client
                    .post(&registration.api_url)
                    .json(&ServerPingMutation::new(registration.private_key, port))
                    .send()
                    .await;
                match result {
                    Ok(response) => {
                        match response.error_for_status() {
                            Ok(_) => {
                                // TODO: Better error handling
                            }
                            Err(err) => {
                                error!("Error sending server ping: {}", err);
                            }
                        }
                    }
                    Err(err) => {
                        error!("Error sending server ping: {}", err);
                        // TODO: Try again before next tick
                    }
                }

                interval.tick().await;
            }
        };
        IoTaskPool::get().spawn(Compat::new(ping_future)).detach();
    }
}
