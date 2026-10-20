use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use phi_core::*;
use phi_proto::{DeviceId, ReactionKind, RevocationTarget};
use std::{
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    process::ExitCode,
};

const SKILL: &str = include_str!("../../../skill/SKILL.md");
mod desktop;
mod output;
#[derive(Parser)]
#[command(
    name = "phi",
    version,
    about = "E2E-encrypted messaging for coding agents"
)]
struct Cli {
    #[arg(long = "as", global = true)]
    handle: Option<String>,
    #[arg(long, global = true, allow_hyphen_values = true)]
    space: Option<String>,
    #[arg(long, global = true)]
    json: bool,
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
        #[arg(allow_hyphen_values = true)]
        space: Option<String>,
    },
    Who,
    Threads {
        destination: Option<String>,
        #[arg(long)]
        closed: bool,
    },
    Channels,
    Spaces,
    Fetch,
    Inbox,
    Read {
        #[arg(allow_hyphen_values = true)]
        thread: MessageRef,
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
        #[arg(allow_hyphen_values = true)]
        thread: MessageRef,
        #[command(flatten)]
        message: MessageArgs,
    },
    React {
        #[arg(allow_hyphen_values = true)]
        message: MessageRef,
        kind: ReactionArg,
    },
    Unread {
        #[arg(allow_hyphen_values = true)]
        thread: MessageRef,
    },
    Close {
        #[arg(allow_hyphen_values = true)]
        thread: MessageRef,
    },
    Reopen {
        #[arg(allow_hyphen_values = true)]
        thread: MessageRef,
    },
    Delete {
        #[arg(allow_hyphen_values = true)]
        thread: MessageRef,
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
    Revoke {
        #[arg(allow_hyphen_values = true)]
        id: DeviceId,
    },
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
    if std::env::args_os().len() == 1 {
        if !desktop::open_installed() {
            eprintln!("Install the Pheidippides desktop app to open it with phi.");
            let _ = Cli::command().print_help();
            println!();
        }
        return ExitCode::SUCCESS;
    }
    let json_requested = std::env::args_os().any(|arg| arg == "--json");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let help = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            if json_requested && !help {
                output::error(&error.to_string(), 1);
            } else {
                let _ = error.print();
            }
            return ExitCode::from(if help { 0 } else { 1 });
        }
    };
    let json = cli.json;
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                output::error(&error.to_string(), error.exit_code());
            } else {
                eprintln!("phi: {error}");
            }
            ExitCode::from(error.exit_code())
        }
    }
}
async fn run(cli: Cli) -> Result<()> {
    if cli.json
        && !matches!(
            &cli.command,
            Command::Fetch
                | Command::Inbox
                | Command::Read { .. }
                | Command::Wait { .. }
                | Command::Threads { .. }
                | Command::Channels
                | Command::Spaces
                | Command::Who
                | Command::Send { .. }
                | Command::Reply { .. }
        )
    {
        return Err(Error::User("--json supports fetch, inbox, read, wait, threads, channels, spaces, who, send and reply".into()));
    }
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
            // Windows accounts are USERNAME; USER is the Unix variable.
            let display = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "person".into());
            let client = Client::create(
                store,
                &server,
                &name,
                &display,
                std::env::var("PHI_ADMIN_TOKEN").ok(),
            )
            .await?;
            println!(
                "created space {} ({})\nnext: phi init {}\n      phi invite",
                client.profile.name, client.profile.space_id, client.profile.name
            );
        }
        Command::Join { link, name } => {
            let client = Client::join(store, &link, &name).await?;
            println!(
                "joined {} · {} · @{}",
                client.profile.name, client.profile.space_id, client.profile.display_name
            );
        }
        Command::Init { space } => {
            let profile = store.bind(&cwd, space.as_deref().or(cli.space.as_deref()))?;
            println!("bound {} to {}", cwd.display(), profile.name);
        }
        Command::Spaces => {
            let binding = Store::binding(&cwd)?;
            let spaces = store.profiles()?.into_iter().map(|p| {
                let bound = binding.as_ref().is_some_and(|b| b.space_id == p.space_id && b.server == p.server);
                serde_json::json!({"name":p.name,"id":p.space_id,"server":p.server,"bound":bound})
            }).collect::<Vec<_>>();
            if cli.json {
                output::print(
                    "spaces",
                    serde_json::json!({"trusted":false,"spaces":spaces}),
                )?;
            } else {
                for p in spaces {
                    println!(
                        "{} · {} · {}{}",
                        p["name"].as_str().unwrap_or(""),
                        p["id"].as_str().unwrap_or(""),
                        p["server"].as_str().unwrap_or(""),
                        if p["bound"] == true {
                            " · bound to this directory"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
        Command::Fetch => {
            let profiles = if cli.space.is_some() || Store::binding(&cwd)?.is_some() {
                vec![store.resolve(&cwd, cli.space.as_deref())?]
            } else {
                store.profiles()?
            };
            let mut first_error = None;
            let mut fetched = Vec::new();
            for profile in profiles {
                let result = async {
                    let mut client =
                        Client::open(store.clone(), profile, cli.handle.as_deref()).await?;
                    if cli.json {
                        let counts = client.counts().await?;
                        let channels = counts.channels.iter().map(|c| serde_json::json!({"id":c.channel_id,"name":client.channel_name(c.channel_id),"unread_threads":c.unread_threads,"mentions":c.mentions})).collect::<Vec<_>>();
                        Ok((String::new(), Some(serde_json::json!({"name":client.profile.name,"id":client.profile.space_id,"handle":client.handle,"person":client.profile.display_name,"unread_threads":counts.unread_threads,"mentions":counts.mentions,"claimed_by_you":counts.claimed_by_you,"channels":channels}))))
                    } else { Ok((client.fetch().await?, None)) }
                }
                .await;
                match result {
                    Ok((output, value)) => {
                        print!("{output}");
                        if let Some(value) = value {
                            fetched.push(value);
                        }
                    }
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
            if cli.json {
                output::print(
                    "fetch",
                    serde_json::json!({"trusted":false,"spaces":fetched}),
                )?;
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
                                    .map(format_last_seen)
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
                Command::Who => {
                    let view = client.who_view().await?;
                    if cli.json {
                        output::print(
                            "who",
                            serde_json::json!({"trusted":false,"people":view.people,"handles":view.handles}),
                        )?;
                    } else {
                        print!("{}", view.text());
                    }
                }
                Command::Threads {
                    destination,
                    closed,
                } => {
                    let entries = client
                        .discover_threads_filtered(destination.as_deref(), closed)
                        .await?;
                    if cli.json {
                        output::print(
                            "threads",
                            serde_json::json!({"threads":entries.iter().map(|e|e.json(&client.names())).collect::<Vec<_>>()}),
                        )?;
                    } else {
                        print!("{}", format_threads(&entries, &client.names()));
                    }
                }
                Command::Channels => {
                    let channels = client.channel_views().await?;
                    if cli.json {
                        output::print(
                            "channels",
                            serde_json::json!({"trusted":false,"channels":channels}),
                        )?;
                    } else {
                        for c in channels {
                            print!("{}", c.text());
                        }
                    }
                }
                Command::Inbox => {
                    let entries = client.inbox().await?;
                    if cli.json {
                        output::print(
                            "inbox",
                            serde_json::json!({"threads":entries.iter().map(|e|e.json(&client.names())).collect::<Vec<_>>()}),
                        )?;
                    } else {
                        print!("{}", format_inbox(&entries, &client.names()));
                    }
                }
                Command::Read { thread, all, last } => {
                    let thread = client.resolve_thread(thread).await?;
                    let view = client.read(thread, ReadOptions { all, last }).await?;
                    if cli.json {
                        output::print("read", serde_json::json!({"thread":view.json()}))?;
                    } else {
                        print!("{}", view.text());
                    }
                }
                Command::Send { to, title, message } => {
                    let options = message.options()?;
                    let message = match client.send(&to, &title, options.clone()).await {
                        Ok(message) => message,
                        Err(error) => {
                            return Err(output::draft_error(
                                &client,
                                error,
                                &options,
                                &["send", &to, &title],
                            )?);
                        }
                    };
                    if let Err(error) = client.message_reference(message.id).await {
                        eprintln!("message committed; numbered reference unavailable: {error}");
                    }
                    if cli.json {
                        output::receipt(&client, &message)?;
                    } else {
                        print!("{}", client.sent_text(&message));
                    }
                }
                Command::Reply { thread, message } => {
                    let thread = client.resolve_thread(thread).await?;
                    let options = message.options()?;
                    let message = match client.reply(thread, options.clone()).await {
                        Ok(message) => message,
                        Err(error) => {
                            return Err(output::draft_error(
                                &client,
                                error,
                                &options,
                                &["reply", &thread.to_string()],
                            )?);
                        }
                    };
                    if let Err(error) = client.message_reference(message.id).await {
                        eprintln!("message committed; numbered reference unavailable: {error}");
                    }
                    if cli.json {
                        output::receipt(&client, &message)?;
                    } else {
                        print!("{}", client.sent_text(&message));
                    }
                }
                Command::React { message, kind } => {
                    let message = client.resolve_message(message).await?;
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
                    let thread = client.resolve_thread(thread).await?;
                    client.unread(thread).await?;
                    println!("marked unread");
                }
                Command::Close { thread } => {
                    let thread = client.resolve_thread(thread).await?;
                    client.close_thread(thread).await?;
                    println!("closed thread {thread}");
                }
                Command::Reopen { thread } => {
                    let thread = client.resolve_thread(thread).await?;
                    client.reopen_thread(thread).await?;
                    println!("reopened thread {thread}");
                }
                Command::Delete { thread } => {
                    let thread = client.resolve_thread(thread).await?;
                    if io::stdin().is_terminal() {
                        eprint!("Delete thread {thread} for everyone? [y/N] ");
                        io::stderr().flush()?;
                        let mut answer = String::new();
                        io::stdin().read_line(&mut answer)?;
                        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                            return Err(Error::User("aborted".into()));
                        }
                    }
                    client.delete_thread(thread).await?;
                    println!("deleted thread {thread}");
                }
                Command::Wait { seconds } => {
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string(&client.wait_json(seconds).await?)?
                        );
                    } else {
                        print!("{}", client.wait_text(seconds).await?);
                    }
                }
                Command::Group { command } => match command {
                    GroupCommand::Create { name, persons } => {
                        let id = client.group_create(&name, &persons).await?;
                        let channel = client
                            .channels
                            .iter()
                            .find(|channel| channel.channel.id == id)
                            .ok_or_else(|| {
                                Error::User(
                                    "created group unavailable in refreshed directory".into(),
                                )
                            })?;
                        let members = channel
                            .channel
                            .members
                            .iter()
                            .map(|person| format!("@{}", client.person_name(*person)))
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("created #{name} ({id}) · members: {members}");
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
                        "{} · {}\nretention_days={}\nsummary_mode={:?}\nsummary_every={}",
                        client.profile.name,
                        client.profile.space_id,
                        client.settings.retention_days,
                        client.settings.summary_mode,
                        client.settings.summary_every
                    ),
                    SpaceCommand::Create { .. } => {
                        unreachable!("handled before profile resolution")
                    }
                },
                Command::Search { text } => {
                    let messages = client.search(&text).await?;
                    print!("{}", format_search(&messages, &client.names()));
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
    fn complete_spec_command_grammar() -> Result<()> {
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
            vec!["init"],
            vec!["who"],
            vec!["threads"],
            vec!["threads", "--closed"],
            vec!["threads", "#all"],
            vec!["threads", "#all", "--closed"],
            vec!["threads", "@bob"],
            vec!["channels", "--json"],
            vec!["spaces"],
            vec!["--json", "wait", "0"],
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
            vec!["close", id],
            vec!["reopen", id],
            vec!["delete", id],
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
        for command in [
            "read", "reply", "react", "unread", "close", "reopen", "delete",
        ] {
            let reference = format!("{id}#2");
            let mut argv = vec!["phi", command, reference.as_str()];
            if command == "react" {
                argv.push("ack");
            }
            assert!(Cli::try_parse_from(argv).is_ok());
        }
        assert!(Cli::try_parse_from(["phi", "react", &format!("{id}#0"), "ack"]).is_err());
        assert!(Cli::try_parse_from(["phi", "space", "create", "game"]).is_err());
        let id = "-AAAAAAAAAAAAAAAAAAAAA";
        let reference = format!("{id}#2");
        for args in [
            vec!["init", id],
            vec!["read", id, "--all"],
            vec!["read", "--all", id],
            vec!["react", &reference, "ack"],
            vec!["reply", &reference, "body"],
            vec!["unread", &reference],
            vec!["close", &reference],
            vec!["reopen", &reference],
            vec!["delete", &reference],
            vec!["device", "revoke", id],
            vec!["--space", id, "who"],
        ] {
            let mut argv = vec!["phi"];
            argv.extend(args);
            assert!(Cli::try_parse_from(argv.clone()).is_ok(), "{argv:?}");
        }
        assert_eq!(
            Cli::try_parse_from(["phi", "--help"])
                .err()
                .map(|error| error.kind()),
            Some(clap::error::ErrorKind::DisplayHelp)
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| Error::User(error.to_string()))?
            .as_secs();
        assert_eq!(format_last_seen(now.saturating_sub(300)), "5 min ago");
        assert_eq!(format_last_seen(now.saturating_sub(7200)), "2 h ago");
        assert_eq!(format_last_seen(0), "1970-01-01 00:00 UTC");
        let directory = tempfile::tempdir()?;
        let bin = directory.path().join("bin");
        let usr = directory.path().join("usr");
        let local = directory.path().join("local");
        let programs = directory.path().join("pf");
        let apps = directory.path().join("Applications");
        std::fs::create_dir_all(&bin)?;
        std::fs::create_dir_all(&usr)?;
        std::fs::create_dir_all(local.join("Pheidippides"))?;
        std::fs::create_dir_all(local.join("Programs").join("Pheidippides"))?;
        std::fs::create_dir_all(programs.join("Pheidippides"))?;
        std::fs::create_dir_all(apps.join("Pheidippides.app"))?;
        for path in [
            bin.join("pheidippides"),
            bin.join("pheidippides.exe"),
            usr.join("pheidippides"),
            local.join("Pheidippides").join("pheidippides.exe"),
            local
                .join("Programs")
                .join("Pheidippides")
                .join("pheidippides.exe"),
            programs.join("Pheidippides").join("pheidippides.exe"),
            apps.join("Pheidippides.AppImage"),
        ] {
            std::fs::write(path, b"test launcher; never executed")?;
        }
        let roots = desktop::Roots {
            local_app_data: Some(local.clone()),
            program_files: Some(programs.clone()),
            application_dirs: vec![apps.clone()],
            system_bin: Some(usr.clone()),
        };
        let path = bin.as_os_str();
        let mac = desktop::candidates("macos", path, &roots);
        assert_eq!(mac[0].program, PathBuf::from("/usr/bin/open"));
        assert_eq!(
            mac[0].args,
            vec!["-a".to_string(), "Pheidippides".to_string()]
        );
        assert_eq!(
            mac[1].args,
            vec![apps.join("Pheidippides.app").to_string_lossy().into_owned()]
        );
        assert_eq!(mac[2].program, bin.join("pheidippides"));
        let linux = desktop::candidates("linux", path, &roots);
        assert_eq!(linux[0].program, bin.join("pheidippides"));
        assert_eq!(linux[1].program, usr.join("pheidippides"));
        assert_eq!(linux[2].program, apps.join("Pheidippides.AppImage"));
        let windows = desktop::candidates("windows", path, &roots);
        assert_eq!(
            windows[0].program,
            local.join("Pheidippides").join("pheidippides.exe")
        );
        assert_eq!(
            windows[1].program,
            local
                .join("Programs")
                .join("Pheidippides")
                .join("pheidippides.exe")
        );
        assert_eq!(
            windows[2].program,
            programs.join("Pheidippides").join("pheidippides.exe")
        );
        assert_eq!(windows[3].program, bin.join("pheidippides.exe"));
        let empty = desktop::Roots {
            local_app_data: None,
            program_files: None,
            application_dirs: Vec::new(),
            system_bin: None,
        };
        assert!(
            desktop::candidates(
                "linux",
                &directory.path().join("missing").into_os_string(),
                &empty
            )
            .is_empty()
        );
        Ok(())
    }
}
