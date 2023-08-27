#![feature(stmt_expr_attributes)]

use serenity::client::EventHandler;
use serenity::json::json;
use serenity::model::application::interaction::Interaction;
use serenity::model::gateway::Ready;
use serenity::prelude::Context;
use serenity::{async_trait, Client};
use tracing::{error, info};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let token = tokio::fs::read_to_string(".token").await.unwrap();

    let intents = {
        use serenity::model::gateway::GatewayIntents as i;

        i::GUILD_MESSAGE_TYPING | i::DIRECT_MESSAGE_TYPING | i::MESSAGE_CONTENT
    };

    Client::builder(token, intents)
        .event_handler(Handler {
            states: States {
                inner: Mutex::new(HashMap::new()),
            },
        })
        .await
        .unwrap()
        .start()
        .await
        .unwrap();
}

struct Handler {
    states: States,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!(
            "serenity is ready, as {} (v: {})",
            ready.user.name, ready.version
        );

        let value = json! {{
            "name": "capture",
            "type": 3,
        }};

        let cmd = ctx
            .http
            .create_guild_application_command(739063162799784006, &value)
            .await
            .unwrap();

        dbg!(cmd);
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Some(interaction) = interaction.as_application_command() else {
            return;
        };

        if interaction.data.name != "capture" {
            return;
        }

        let message = interaction
            .data
            .resolved
            .messages
            .values()
            .collect::<Vec<_>>()[0];

        let user_id = interaction.user.id;
        let message_id = message.id;

        if let Some([start_id, end_id]) = self.states.process(user_id, message_id).await {
            use std::pin::pin;

            use serenity::futures::StreamExt;

            let mut messages = Vec::new();
            let mut collecting = false;

            let mut stream = pin!(interaction.channel_id.messages_iter(&ctx.http));
            while let Some(Ok(msg)) = stream.next().await {
                #[rustfmt::skip]
                match (collecting, msg.id == end_id, msg.id == start_id) {
                    (false, false, false) => { /* skip message */ },

                    (false,  true, false) => {
                        collecting = true;
                        messages.push(msg);
                    },
                    ( true, false, false) => {
                        messages.push(msg);
                    },
                    ( true, false,  true) => {
                        collecting = false;
                        messages.push(msg);
                        break;
                    },

                    (false, false,  true) => panic!("start before end"),
                    ( true,  true, false) => panic!("end after end"),
                    (    _,  true,  true) => panic!("start and end"),
                }
            }

            if collecting {
                panic!("not found end message");
            }

            let content = messages
                .into_iter()
                .rev()
                .map(|msg| format!("- {}\n", msg.content.chars().take(10).collect::<String>()))
                .collect::<String>();

            interaction
                .create_interaction_response(&ctx.http, |builder| {
                    builder
                        .kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|data| data.content(content))
                })
                .await
                .unwrap();
        } else {
            interaction
                .create_interaction_response(&ctx.http, |builder| {
                    builder
                        .kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|data| {
                            let message_head = if message.content.chars().count() > 10 {
                                message
                                    .content
                                    .chars()
                                    .take(10)
                                    .chain(['.', '.', '.'])
                                    .collect::<String>()
                            } else {
                                message.content.clone()
                            };

                            data.ephemeral(true)
                                .content(format!("capture starting at \"{message_head}\""))
                        })
                })
                .await
                .unwrap();
        }
    }
}

use std::collections::HashMap;

use serenity::model::application::interaction::InteractionResponseType;
use serenity::model::id::{MessageId, UserId};
use tokio::sync::Mutex;

struct States {
    inner: Mutex<HashMap<UserId, MessageId>>,
}

impl States {
    async fn take(&self, key: UserId) -> Option<MessageId> { self.inner.lock().await.remove(&key) }

    async fn insert(&self, key: UserId, val: MessageId) -> Option<MessageId> {
        self.inner.lock().await.insert(key, val)
    }

    async fn process(&self, key: UserId, val: MessageId) -> Option<[MessageId; 2]> {
        let Some(id0) = self.insert(key, val).await else {
            return None;
        };

        let Some(id1) = self.take(key).await else {
            unreachable!();
        };

        Some([id0, id1])
    }
}
