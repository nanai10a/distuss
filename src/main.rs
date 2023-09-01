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
            printer: Printer::new(),
            states: States::new(),
        })
        .await
        .unwrap()
        .start()
        .await
        .unwrap();
}

struct Handler {
    printer: Printer,
    states: States,
}

struct Printer {
    browser: headless_chrome::Browser,
    dir: std::path::PathBuf,
}

impl Printer {
    fn new() -> Self {
        let opts = headless_chrome::LaunchOptions {
            headless: true,
            ..Default::default()
        };

        let browser = headless_chrome::Browser::new(opts).unwrap();

        // for keeping to alive browser window
        // in headless, initially tabs are only 1, consumes when first request
        browser.new_tab().unwrap();

        Self {
            browser,
            dir: std::env::temp_dir(),
        }
    }
}

use serenity::http::{CacheHttp, Http};
use serenity::model::channel::Message;
use serenity::model::guild::Role;
use serenity::model::id::GuildId;
use serenity::model::user::User;

#[deprecated(note = "this search method makes no sense")]
async fn find_role(http: &Http, user: &User, guild_id: Option<GuildId>) -> Option<Role> {
    let Some(guild_id) = guild_id else {
        // message isn't in guild, fallback
        return None;
    };

    // get roles in order of hierarchy, skip `@everyone`
    let mut roles = http
        .get_guild_roles(guild_id.0)
        .await
        .unwrap()
        .into_iter()
        .skip(1);

    loop {
        let Some(role) = roles.next() else {
            // user has no role, fallback
            break None;
        };

        let has_role = user.has_role(http, guild_id, role.id).await.unwrap();

        if !has_role {
            // user doesn't have this role, continuing find
            continue;
        }

        // found role! return
        break Some(role);
    }
}

async fn message_to_html(cache_http: impl CacheHttp, msg: &Message) -> String {
    use dioxus::prelude::*;

    let content = {
        use pulldown_cmark::html::push_html;
        use pulldown_cmark::Parser;

        let mut html = String::new();
        push_html(&mut html, Parser::new(&msg.content));

        html
    };

    let avatar = msg.author.avatar_url().unwrap_or_default();

    // not working
    let username = msg
        .author_nick(&cache_http)
        .await
        .unwrap_or_else(|| msg.author.name.clone());

    let role = find_role(cache_http.http(), &msg.author, msg.guild_id).await;

    // may working
    let role_icon = role
        .as_ref()
        .and_then(|role| role.icon.as_ref().map(|hash| (role.id, hash)))
        .map(|(id, hash)| format!("https://cdn.discordapp.com/role-icons/{id}/{hash}.webp"))
        .unwrap_or_default();

    // not working
    let username_style = role
        .map(|role| format!("color:#{}", role.colour.hex()))
        .unwrap_or_default();

    // format of ja_JP
    let timestamp = msg.timestamp.format("%Y/%m/%d %a, %T%.3f (%Z)").to_string();

    struct Props {
        avatar: String,
        username: String,
        username_style: String,
        role_icon: String,
        timestamp: String,
        content: String,
    }

    let mut vdom = VirtualDom::new_with_props(
        |cx| {
            render! {
              div { class: "message",
                div { class: "contents",
                  img { class: "avatar", src: &*cx.props.avatar }
                  h3 { class: "header",
                    span { class: "username", span { style: &*cx.props.username_style, &*cx.props.username } img { src: &*cx.props.role_icon } }
                    span { class: "timestamp", time { &*cx.props.timestamp } }
                  }
                  div { class: "content", dangerous_inner_html: &*cx.props.content }
                }
              }
            }
        },
        Props {
            avatar,
            username,
            username_style,
            role_icon,
            timestamp,
            content,
        },
    );

    let _ = vdom.rebuild();
    dioxus_ssr::render(&vdom)
}

async fn messages_to_html(
    cache_http: impl CacheHttp,
    msgs: impl Iterator<Item = &Message>,
) -> String {
    use dioxus::prelude::*;

    let mut list = Vec::new();
    for msg in msgs {
        list.push(message_to_html(&cache_http, msg).await);
    }

    let mut vdom = VirtualDom::new_with_props(
        |cx| {
            render! {
              head {
                meta { charset: "utf-8" }
                meta { name: "viewport", content: "width=device-width,initial-scale=1" }
                link { rel: "stylesheet", href: "https://cdn.jsdelivr.net/npm/normalize.css@8.0.1/normalize.css" }
              }
              body {
                style { dangerous_inner_html: include_str!("../dark.css") }

                ol { id: "list",
                  cx.props.iter().map(|html| {
                    rsx! { li { class: "item", dangerous_inner_html: &**html } }
                  })
                }
              }
            }
        },
        list,
    );

    let _ = vdom.rebuild();
    format!("<!DOCTYPE html><html>{}</html>", dioxus_ssr::render(&vdom))
}

impl Printer {
    fn alloc_file(&self) -> std::path::PathBuf {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();

        let mut path = self.dir.clone();
        path.push(format!("{:016x}.html", now.as_nanos()));

        path
    }

    async fn print(
        &self,
        cache_http: impl CacheHttp,
        msgs: impl Iterator<Item = &Message>,
    ) -> Vec<u8> {
        use headless_chrome::protocol::cdp::Emulation::SetDeviceMetricsOverride;
        use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
        use headless_chrome::protocol::cdp::Target::CreateTarget;

        let loc = self.alloc_file();
        let html = messages_to_html(cache_http, msgs).await;

        std::fs::write(&loc, html).unwrap();

        let tab = self
            .browser
            .new_tab_with_options(CreateTarget {
                url: format!("file://{}", loc.to_string_lossy()),
                width: None,
                height: None,
                browser_context_id: None,
                enable_begin_frame_control: None,
                new_window: None,
                background: None,
            })
            .unwrap();

        tab.call_method(SetDeviceMetricsOverride {
            width: 700,
            height: 1920,
            device_scale_factor: 2.0,
            mobile: false,

            scale: None,
            screen_width: None,
            screen_height: None,
            position_x: None,
            position_y: None,
            dont_set_visible_size: None,
            screen_orientation: None,
            viewport: None,
            display_feature: None,
        })
        .unwrap();

        let elem = tab.wait_for_element("body").unwrap();
        let vp = elem.get_box_model().unwrap().margin_viewport();

        let image = tab
            .capture_screenshot(CaptureScreenshotFormatOption::Png, None, Some(vp), true)
            .unwrap();

        tab.close_with_unload().unwrap();
        std::fs::remove_file(loc).unwrap();

        image
    }
}

impl States {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
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
            interaction
                .create_interaction_response(&ctx.http, |builder| {
                    builder
                        .kind(InteractionResponseType::ChannelMessageWithSource)
                        .interaction_response_data(|data| {
                            data.ephemeral(true).content("unknown command")
                        })
                })
                .await
                .unwrap();

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

            interaction.defer_ephemeral(&ctx).await.unwrap();

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

            let image = self.printer.print(&ctx, messages.iter().rev()).await;

            interaction
                .create_followup_message(&ctx.http, |builder| {
                    builder.add_file((&*image, "capture.png"))
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
