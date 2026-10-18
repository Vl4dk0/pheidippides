#[tokio::test(flavor = "multi_thread")]
async fn cli_workflow_references_wait_skill_and_exit_codes() -> phi_core::Result<()> {
    use std::process::Command;
    let binary = env!("CARGO_BIN_EXE_phi");
    let output = Command::new(binary).arg("skill").output()?;
    assert!(output.status.success());
    assert_eq!(output.stdout, include_bytes!("../../../skill/SKILL.md"));
    let dir = tempfile::tempdir()?;
    let output = Command::new(binary)
        .args(["skill", "--install"])
        .arg(dir.path())
        .output()?;
    assert!(output.status.success());
    assert_eq!(
        std::fs::read(dir.path().join("SKILL.md"))?,
        include_bytes!("../../../skill/SKILL.md")
    );
    assert!(
        Command::new(binary)
            .arg("--help")
            .output()?
            .status
            .success()
    );
    let output = Command::new(binary)
        .args(["react", "invalid-id", "claim"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    let workspace = tempfile::tempdir()?;
    let server = phi_server::Server::open(phi_server::config::Config {
        data_dir: workspace.path().join("server"),
        allow_space_creation: phi_server::config::Creation::Anyone,
        ..Default::default()
    })
    .map_err(|e| phi_core::Error::User(e.to_string()))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let router = server.router();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    struct Abort(tokio::task::JoinHandle<()>);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server_task = Abort(task);
    let created = phi(
        binary,
        workspace.path(),
        &["space", "create", "game", "--server", &url],
    )
    .await?;
    assert!(
        created.starts_with("created space game (")
            && created.contains("phi init game")
            && created.contains("phi invite")
    );
    let space = created
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("created space game ("))
        .and_then(|id| id.strip_suffix(')'))
        .ok_or_else(|| phi_core::Error::User("missing created space ID".into()))?;
    phi(binary, workspace.path(), &["init", space]).await?;
    let sent = phi(
        binary,
        workspace.path(),
        &[
            "send",
            "#all",
            "Task",
            "first body",
            "--summary",
            "First summary",
        ],
    )
    .await?;
    let thread = sent
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| phi_core::Error::User("missing thread ID".into()))?;
    let root = format!("{thread}#1");
    assert!(sent.contains(&format!("message {root}")));
    phi(binary, workspace.path(), &["react", &root, "ack"]).await?;
    let replied = phi(
        binary,
        workspace.path(),
        &["reply", &root, "second body", "--summary", "Both messages"],
    )
    .await?;
    let reply = format!("{thread}#2");
    assert!(replied.contains(&format!("message {reply}")));
    let raw_reply = replied
        .split_whitespace()
        .last()
        .ok_or_else(|| phi_core::Error::User("missing message ID".into()))?;
    let read = phi(binary, workspace.path(), &["read", &reply, "--last", "1"]).await?;
    assert!(
        read.contains("summary (as of #2; up to date)")
            && read.contains("(you · agent)")
            && read.contains(&reply)
            && !read.contains("UNTRUSTED")
    );
    phi(binary, workspace.path(), &["react", &reply, "claim"]).await?;
    assert!(
        phi(binary, workspace.path(), &["wait", "0"])
            .await?
            .is_empty()
    );
    let events = phi(binary, workspace.path(), &["--as", "observer", "wait", "0"]).await?;
    assert!(
        events.contains("new message")
            && events.contains("reply in thread")
            && events.contains("reaction: claim by alice/main")
            && events.contains(&reply)
            && events.contains(&format!("phi read {thread}"))
    );
    assert!(!events.contains("claimed by"));
    phi(binary, workspace.path(), &["unread", &reply]).await?;
    assert!(
        phi(binary, workspace.path(), &["read", raw_reply])
            .await?
            .contains(&reply)
    );
    let search = phi(binary, workspace.path(), &["search", "second body"]).await?;
    assert!(
        search.contains(&reply)
            && search.contains("(you · agent)")
            && !search.contains("UNTRUSTED")
    );
    Ok(())
}

async fn phi(binary: &str, root: &std::path::Path, args: &[&str]) -> phi_core::Result<String> {
    let home = root.join("home");
    std::fs::create_dir_all(&home)?;
    let mut command = tokio::process::Command::new(binary);
    if !args.contains(&"--as") {
        command.args(["--as", "main"]);
    }
    let output = command
        .args(args)
        .current_dir(root)
        .env("HOME", &home)
        .env("USER", "alice")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("appdata"))
        .env("LOCALAPPDATA", root.join("localappdata"))
        .env_remove("PHI_ADMIN_TOKEN")
        .output()
        .await?;
    if !output.status.success() {
        return Err(phi_core::Error::User(format!(
            "phi {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout).map_err(|e| phi_core::Error::User(e.to_string()))
}
