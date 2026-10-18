use clap::{Args, Parser, Subcommand, ValueEnum};
use phi_core::*;
use phi_proto::{DeviceId, MessageId, ReactionKind, RevocationTarget};
use std::{
    io::{self, Read},
    path::PathBuf,
    process::ExitCode,
};

const SKILL: &str = include_str!("../../../skill/SKILL.md");
#[derive(Parser)]
#[command(
    name = "phi",
    version,
    about = "E2E-encrypted messaging for coding agents"
)]
struct Cli {
    #[arg(long = "as", global = true)]
    handle: Option<String>,
    #[arg(long, global = true)]
    space: Option<String>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Space {
        #[command(subcommand)]
        command: SpaceCommand,
    },
    Invite {
        #[arg(long)]
        note: Option<String>,
    },
    Join {
        link: String,
        #[arg(long)]
        name: String,
    },
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    Init {
        space: String,
    },
    Who,
    Fetch,
    Inbox,
    Read {
        thread: MessageId,
        #[arg(long, conflicts_with = "last")]
        all: bool,
        #[arg(long)]
        last: Option<usize>,
    },
    Send {
        to: String,
        title: String,
        #[command(flatten)]
        message: MessageArgs,
    },
    Reply {
        thread: MessageId,
        #[command(flatten)]
        message: MessageArgs,
    },
    React {
        message: MessageId,
        kind: ReactionArg,
    },
    Unread {
        thread: MessageId,
    },
    Wait {
        #[arg(default_value_t = 600)]
        seconds: u16,
    },
    Group {
        #[command(subcommand)]
        command: GroupCommand,
    },
    Remove {
        person: String,
    },
    Search {
        text: String,
    },
    Skill {
        #[arg(long)]
        install: Option<PathBuf>,
    },
}
#[derive(Args)]
struct MessageArgs {
    body: Option<String>,
    #[arg(long = "file")]
    files: Vec<PathBuf>,
    #[arg(long)]
    summary: Option<String>,
}
impl MessageArgs {
    fn options(self) -> Result<SendOptions> {
        let body = match self.body.as_deref() {
            None | Some("-") => {
                let mut body = String::new();
                io::stdin().take(256 * 1024 + 1).read_to_string(&mut body)?;
                body
            }
            Some(_) => self.body.unwrap_or_default(),
        };
        Ok(SendOptions {
            body,
            summary: self.summary,
            files: self.files,
        })
    }
}
#[derive(Subcommand)]
enum SpaceCommand {
    Create {
        name: String,
        #[arg(long)]
        server: String,
    },
    Set {
        key: String,
        value: String,
    },
    Show,
}
#[derive(Subcommand)]
enum DeviceCommand {
    Add,
    List,
    Revoke { id: DeviceId },
}
#[derive(Subcommand)]
enum GroupCommand {
    Create { name: String, persons: Vec<String> },
    Add { name: String, person: String },
    Remove { name: String, person: String },
}
#[derive(Clone, Copy, ValueEnum)]
enum ReactionArg {
    Up,
    Down,
    Ack,
    Claim,
    Unclaim,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let help = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            let _ = error.print();
            return ExitCode::from(if help { 0 } else { 1 });
        }
    };
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("phi: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
async fn run(cli: Cli) -> Result<()> {
    if let Command::Skill { install } = &cli.command {
        if let Some(dir) = install {
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join("SKILL.md"), SKILL)?;
            println!("installed {}", dir.join("SKILL.md").display());
        } else {
            print!("{SKILL}");
        }
        return Ok(());
    }
    let store = Store::platform()?;
    let cwd = std::env::current_dir()?;
    match cli.command {
        Command::Space {
            command: SpaceCommand::Create { name, server },
        } => {
            let display = std::env::var("USER").unwrap_or_else(|_| "person".into());
            let client = Client::create(
                store,
                &server,
                &name,
                &display,
                std::env::var("PHI_ADMIN_TOKEN").ok(),
            )
            .await?;
            println!("{}", client.profile.space_id);
        }
        Command::Join { link, name } => {
            let client = Client::join(store, &link, &name).await?;
            println!(
                "joined {} · {} · @{}",
                client.profile.name, client.profile.space_id, client.profile.display_name
            );
        }
        Command::Init { space } => {
            let profile = store.bind(&cwd, &space)?;
            println!("bound {} to {}", cwd.display(), profile.name);
        }
        Command::Fetch => {
            let profiles = if cli.space.is_some() || Store::binding(&cwd)?.is_some() {
                vec![store.resolve(&cwd, cli.space.as_deref())?]
            } else {
                store.profiles()?
            };
            let mut first_error = None;
            for profile in profiles {
                let result = async {
                    let mut client =
                        Client::open(store.clone(), profile, cli.handle.as_deref()).await?;
                    client.fetch().await
                }
                .await;
                match result {
                    Ok(output) => print!("{output}"),
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
            if let Some(error) = first_error {
                return Err(error);
            }
        }
        command => {
            let profile = store.resolve(&cwd, cli.space.as_deref())?;
            let mut client = Client::open(store, profile, cli.handle.as_deref()).await?;
            match command {
                Command::Invite { note } => {
                    if let Some(note) = note {
                        eprintln!("invite note: {note}");
                    }
                    println!("{}", client.invite(false, false).await?.as_str());
                }
                Command::Device { command } => match command {
                    DeviceCommand::Add => {
                        println!("{}", client.invite(true, false).await?.as_str())
                    }
                    DeviceCommand::List => {
                        for device in &client.devices {
                            println!(
                                "{} · @{} · {:?} · last seen {}{}",
                                device.cert.data.device_id,
                                client.person_name(device.cert.data.person_id),
                                device.cert.data.kind,
                                device
                                    .last_seen_at
                                    .map(|t| t.to_string())
                                    .unwrap_or_else(|| "never".into()),
                                if device.revoked_at.is_some() {
                                    " · revoked"
                                } else {
                                    ""
                                }
                            );
                        }
                    }
                    DeviceCommand::Revoke { id } => {
                        client
                            .revoke(RevocationTarget::Device { device_id: id })
                            .await?;
                        println!("device revoked; affected channels rotated");
                    }
                },
                Command::Who => print!("{}", client.who().await?),
                Command::Inbox => {
                    for entry in client.inbox().await? {
                        print!("{}", entry.text(&client.names()));
                    }
                }
                Command::Read { thread, all, last } => print!(
                    "{}",
                    client.read(thread, ReadOptions { all, last }).await?.text()
                ),
                Command::Send { to, title, message } => {
                    let message = client.send(&to, &title, message.options()?).await?;
                    println!("{} · #{}", message.id, message.seq);
                }
                Command::Reply { thread, message } => {
                    let message = client.reply(thread, message.options()?).await?;
                    println!("{} · #{}", message.id, message.seq);
                }
                Command::React { message, kind } => {
                    let (kind, remove) = match kind {
                        ReactionArg::Up => (ReactionKind::Up, false),
                        ReactionArg::Down => (ReactionKind::Down, false),
                        ReactionArg::Ack => (ReactionKind::Ack, false),
                        ReactionArg::Claim => (ReactionKind::Claim, false),
                        ReactionArg::Unclaim => (ReactionKind::Claim, true),
                    };
                    client.react(message, kind, remove).await?;
                    println!("reaction recorded");
                }
                Command::Unread { thread } => {
                    client.unread(thread).await?;
                    println!("marked unread");
                }
                Command::Wait { seconds } => {
                    for event in client.wait(seconds).await? {
                        match event {
                            WaitEntry::Message(entry) => print!("{}", entry.text(&client.names())),
                            WaitEntry::Reaction(reaction) => println!(
                                "{} · {}{}",
                                reaction.data.message_id,
                                if reaction.data.remove { "removed " } else { "" },
                                format_reactions(std::slice::from_ref(&reaction), &client.names())
                            ),
                            WaitEntry::Settings(change) => println!(
                                "#all · space settings changed: retention_days={} summary_mode={:?}",
                                change.data.settings.retention_days,
                                change.data.settings.summary_mode
                            ),
                            WaitEntry::Revocation(revocation) => {
                                println!("#all · signed revocation: {:?}", revocation.data.target)
                            }
                        }
                    }
                }
                Command::Group { command } => match command {
                    GroupCommand::Create { name, persons } => {
                        println!("{}", client.group_create(&name, &persons).await?);
                    }
                    GroupCommand::Add { name, person } => {
                        client.group_member(&name, &person, false).await?;
                        println!("member added; channel rotated");
                    }
                    GroupCommand::Remove { name, person } => {
                        client.group_member(&name, &person, true).await?;
                        println!("member removed; channel rotated");
                    }
                },
                Command::Remove { person } => {
                    let person_id = client.person(&person)?;
                    client
                        .revoke(RevocationTarget::Person { person_id })
                        .await?;
                    println!("person removed; affected channels rotated");
                }
                Command::Space { command } => match command {
                    SpaceCommand::Set { key, value } => {
                        client.set_settings(&key, &value).await?;
                        println!("space setting updated");
                    }
                    SpaceCommand::Show => println!(
                        "{} · {}\nretention_days={}\nsummary_mode={:?}",
                        client.profile.name,
                        client.profile.space_id,
                        client.settings.retention_days,
                        client.settings.summary_mode
                    ),
                    SpaceCommand::Create { .. } => {
                        unreachable!("handled before profile resolution")
                    }
                },
                Command::Search { text } => {
                    for message in client.search(&text).await? {
                        println!(
                            "thread {} · message {}",
                            message
                                .message
                                .envelope
                                .thread_id
                                .unwrap_or(message.message.id),
                            message.message.id
                        );
                        print!("{}", format_message(&message, &client.names()));
                    }
                }
                _ => unreachable!("handled before profile resolution"),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_spec_command_grammar() {
        let id = "AAAAAAAAAAAAAAAAAAAAAA";
        let cases: Vec<Vec<&str>> = vec![
            vec![
                "space",
                "create",
                "game",
                "--server",
                "http://localhost:8443",
            ],
            vec!["space", "set", "retention", "30"],
            vec!["space", "show"],
            vec!["invite", "--note", "for Bob"],
            vec!["join", "phi://host/code#secret", "--name", "bob"],
            vec!["device", "add"],
            vec!["device", "list"],
            vec!["device", "revoke", id],
            vec!["init", "game"],
            vec!["who"],
            vec!["fetch"],
            vec!["inbox"],
            vec!["read", id],
            vec!["read", id, "--all"],
            vec!["read", id, "--last", "2"],
            vec![
                "send",
                "@bob/reviewer",
                "Review",
                "-",
                "--file",
                "one.png",
                "--file",
                "two.txt",
                "--summary",
                "Whole thread",
            ],
            vec!["reply", id, "body", "--summary", "Whole thread"],
            vec!["react", id, "up"],
            vec!["react", id, "down"],
            vec!["react", id, "ack"],
            vec!["react", id, "claim"],
            vec!["react", id, "unclaim"],
            vec!["unread", id],
            vec!["wait"],
            vec!["wait", "5"],
            vec!["group", "create", "backend", "@alice", "@bob"],
            vec!["group", "add", "backend", "@bob"],
            vec!["group", "remove", "backend", "@bob"],
            vec!["remove", "@bob"],
            vec!["search", "needle"],
            vec!["skill"],
            vec!["skill", "--install", "/tmp/skill"],
        ];
        for args in cases {
            let mut argv = vec!["phi", "--space", "game", "--as", "reviewer"];
            argv.extend(args);
            assert!(Cli::try_parse_from(argv.clone()).is_ok(), "{argv:?}");
        }
        assert!(Cli::try_parse_from(["phi", "read", id, "--all", "--last", "2"]).is_err());
        assert!(Cli::try_parse_from(["phi", "join", "link"]).is_err());
    }
}
