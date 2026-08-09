//! `spacebot chat` — interactive portal conversation with an agent.
//!
//! Messages go out through `POST /portal/send`; replies arrive on the
//! global SSE event bus, filtered to this conversation. Deltas are skipped
//! in favor of the final `outbound_message` event so lines print whole.

use anyhow::Context as _;
use clap::Args;
use futures::StreamExt as _;
use tokio::io::AsyncBufReadExt as _;

#[derive(Args)]
pub struct ChatArgs {
    /// Agent ID (defaults to the first configured agent)
    #[arg(short, long)]
    agent: Option<String>,

    /// Resume an existing conversation session
    #[arg(short, long)]
    session: Option<String>,

    /// Sender display name
    #[arg(long, default_value = "user")]
    name: String,
}

pub async fn run(ctx: &super::Context, args: ChatArgs) -> anyhow::Result<()> {
    let client = super::client::ApiClient::from_context(ctx)?;

    let agent_id = match &args.agent {
        Some(id) => id.clone(),
        None => {
            let value = client.get("agents").await?;
            value["agents"][0]["id"]
                .as_str()
                .map(str::to_string)
                .context("no agents available — is one configured?")?
        }
    };

    let resuming = args.session.is_some();
    let session_id = args
        .session
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if resuming {
        let history = client
            .get(&format!(
                "portal/history?agent_id={}&session_id={}&limit=20",
                urlencoding::encode(&agent_id),
                urlencoding::encode(&session_id)
            ))
            .await?;
        if let Some(messages) = history.as_array() {
            for message in messages {
                let role = message["role"].as_str().unwrap_or("");
                let content = message["content"].as_str().unwrap_or("");
                println!("{role}: {content}");
            }
        }
    }

    eprintln!("Chatting with {agent_id} (session: {session_id})");
    eprintln!("Ctrl-D or /quit to exit.");

    // Print replies for this conversation as they arrive on the event bus.
    let events = client.stream("events").await?;
    let listener_agent = agent_id.clone();
    let listener_session = session_id.clone();
    let listener = tokio::spawn(async move {
        let mut stream = events.bytes_stream();
        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    eprintln!("event stream error: {error}");
                    break;
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(newline) = buffer.find('\n') {
                let line = buffer[..newline].trim_end_matches('\r').to_string();
                buffer.drain(..=newline);
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };
                if event["type"] != "outbound_message"
                    || event["agent_id"] != listener_agent.as_str()
                {
                    continue;
                }
                let channel_id = event["channel_id"].as_str().unwrap_or("");
                if channel_id != listener_session && !channel_id.ends_with(&listener_session) {
                    continue;
                }
                if let Some(text) = event["text"].as_str() {
                    println!("{text}");
                    eprint!("> ");
                }
            }
        }
        eprintln!("(event stream closed)");
    });

    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    eprint!("> ");
    while let Some(line) = lines.next_line().await? {
        let message = line.trim();
        if message == "/quit" {
            break;
        }
        if message.is_empty() {
            eprint!("> ");
            continue;
        }
        client
            .post(
                "portal/send",
                &serde_json::json!({
                    "agent_id": agent_id,
                    "session_id": session_id,
                    "sender_name": args.name,
                    "message": message,
                }),
            )
            .await?;
    }

    listener.abort();
    eprintln!("Session: {session_id} (resume with `spacebot chat --session {session_id}`)");
    Ok(())
}
