use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, IsTerminal, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
};

#[derive(Args)]
pub struct Exec {
    /// Message to send. Use - or omit to read stdin.
    prompt: Option<String>,
    /// Continue a saved Sol chat by ID.
    #[arg(long)]
    resume: Option<String>,
    /// Stream Codex-style JSON Lines to stdout.
    #[arg(long)]
    json: bool,
}

#[path = "conversation_web.rs"]
mod web;

fn request(body: Value) -> io::Result<BufReader<UnixStream>> {
    let exe = std::env::current_exe()?.canonicalize()?;
    super::initialize(&exe)?;
    let state = fs::read_to_string(exe.with_file_name("hey-boss.state"))?;
    let state = Path::new(state.trim_end());
    let mut stream = if exe.with_file_name("hey-boss.companion").exists() {
        super::broker::connect_bridge(state, true).map_err(io::Error::other)?
    } else {
        UnixStream::connect(state.join("daemon.sock"))?
    };
    serde_json::to_writer(&mut stream, &body)?;
    stream.shutdown(Shutdown::Write)?;
    Ok(BufReader::new(stream))
}
impl Exec {
    pub fn run(&self) -> io::Result<()> {
        let mut prompt = self.prompt.clone().filter(|s| s != "-").unwrap_or_default();
        if !io::stdin().is_terminal() || self.prompt.as_deref() == Some("-") {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            if !input.is_empty() {
                if !prompt.is_empty() {
                    prompt.push_str("\n\n");
                }
                prompt.push_str(&input);
            }
        }
        if prompt.trim().is_empty() {
            return Err(io::Error::other("Supply a message or pipe it on stdin"));
        }
        let reader = request(
            json!({"command":"chat_exec","sync":true,"chat_id":self.resume,"question":prompt}),
        )?;
        let mut last = String::new();
        for line in reader.lines() {
            let line = line?;
            let event: Value = serde_json::from_str(&line)?;
            if let Some(error) = event["error"].as_str() {
                return Err(io::Error::other(error.to_string()));
            }
            if self.json {
                writeln!(io::stdout().lock(), "{line}")?;
            }
            match event["type"].as_str() {
                Some("thread.started") if !self.json => {
                    eprintln!("Chat {}", event["thread_id"].as_str().unwrap_or(""))
                }
                Some("item.started") if !self.json => eprintln!(
                    "{}",
                    event["item"]["command"].as_str().unwrap_or("Using tool…")
                ),
                Some("item.completed") if event["item"]["type"] == "agent_message" => {
                    last = event["item"]["text"].as_str().unwrap_or("").into()
                }
                Some("turn.completed") => {
                    if !self.json {
                        writeln!(io::stdout().lock(), "{last}")?;
                    }
                    return Ok(());
                }
                Some("turn.failed") => {
                    return Err(io::Error::other(
                        event["error"]["message"]
                            .as_str()
                            .unwrap_or("Turn failed")
                            .to_owned(),
                    ));
                }
                _ => {}
            }
        }
        Err(io::Error::other(
            "Desktop disconnected before the turn finished",
        ))
    }
}

#[derive(Subcommand)]
pub enum Chats {
    /// Serve Conversations over paired HTTPS on this Mac.
    Web {
        /// Set up Wi-Fi HTTPS and start the service at login.
        #[arg(long)]
        install: bool,
        /// Print the existing pairing code without restarting the service.
        #[arg(long, conflicts_with = "install")]
        pairing_code: bool,
    },
    /// Show one page, newest first.
    List {
        #[arg(long)]
        archived: bool,
        /// Start after the last ID of the previous page.
        #[arg(long)]
        after: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Print the chat transcript without switching the desktop window.
    Read {
        id: String,
    },
    Rename {
        id: String,
        title: String,
    },
    Archive {
        id: String,
    },
    Restore {
        id: String,
    },
    /// Permanently delete this chat and its local recordings and media.
    Delete {
        id: String,
    },
}

impl Chats {
    pub fn run(&self) -> io::Result<()> {
        let body = match self {
            Self::Web {
                install,
                pairing_code,
            } => return web::run(*install, *pairing_code),
            Self::List {
                archived, after, ..
            } => {
                json!({"command":"chat_list","archived":archived,"chat_id":after})
            }
            Self::Read { id } => json!({"command":"chat_read","chat_id":id}),
            Self::Rename { id, title } => {
                json!({"command":"chat_rename","chat_id":id.to_string(),"title":title})
            }
            Self::Archive { id } => json!({"command":"chat_archive","chat_id":id.to_string()}),
            Self::Restore { id } => json!({"command":"chat_restore","chat_id":id.to_string()}),
            Self::Delete { id } => json!({"command":"chat_delete","chat_id":id.to_string()}),
        };
        let mut body = body;
        body["sync"] = false.into();
        let result: Value = serde_json::from_reader(request(body)?)?;
        if result["status"] != "ok" {
            return Err(io::Error::other(
                result["error"]
                    .as_str()
                    .unwrap_or("Desktop chat request failed")
                    .to_owned(),
            ));
        }
        if let Self::Read { .. } = self {
            println!(
                "{}",
                result["transcript"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("Missing transcript"))?
            );
        } else if let Self::List { json, .. } = self {
            if *json {
                println!("{result}");
            } else {
                for chat in result["chats"]
                    .as_array()
                    .ok_or_else(|| io::Error::other("Missing chats"))?
                {
                    println!(
                        "{}  {}",
                        chat["id"].as_str().unwrap_or(""),
                        chat["title"].as_str().unwrap_or("")
                    );
                }
                if let Some(after) = result["next"].as_str() {
                    eprintln!("More chats: --after {after}");
                }
            }
        }
        Ok(())
    }
}
