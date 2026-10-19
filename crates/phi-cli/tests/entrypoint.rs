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
    let phase = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let router_phase = phase.clone();
    let router = server.router().layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let phase = router_phase.clone();
            async move {
                use axum::response::IntoResponse;
                use std::sync::atomic::Ordering;
                let messages = request.uri().path().starts_with("/v1/channels/")
                    && request.uri().path().ends_with("/messages");
                let sent = messages && request.method() == axum::http::Method::POST;
                if messages
                    && request.method() == axum::http::Method::GET
                    && phase.load(Ordering::SeqCst) == 2
                {
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(serde_json::json!({"error":"history unavailable after commit"})),
                    )
                        .into_response();
                }
                let response = next.run(request).await;
                if sent && response.status().is_success() {
                    let _ = phase.compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst);
                }
                response
            }
        },
    ));
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
    phi(binary, workspace.path(), &["init"]).await?;
    assert!(std::fs::read_to_string(workspace.path().join(".phi"))?.contains(space));
    let body = "first body: `code` and \"quotes\" and $HOME and $(false)";
    let sent = phi_with_input(
        binary,
        workspace.path(),
        &[
            "send",
            "@alice/observer",
            "Task",
            "-",
            "--summary",
            "First summary",
        ],
        Some(body),
    )
    .await?;
    let thread = sent
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| phi_core::Error::User("missing thread ID".into()))?;
    let root = format!("{thread}#1");
    assert!(sent.contains(&format!("message {root}")));
    let group = phi(
        binary,
        workspace.path(),
        &["group", "create", "team", "@alice", "@alice"],
    )
    .await?;
    assert!(group.starts_with("created #team (") && group.ends_with(") · members: @alice\n"));
    let who = phi(binary, workspace.path(), &["who"]).await?;
    let devices = phi(binary, workspace.path(), &["device", "list"]).await?;
    assert!(who.contains("last seen just now") && devices.contains("last seen just now"));
    let read_body = phi(binary, workspace.path(), &["read", &root]).await?;
    assert!(read_body.contains(body));
    phi(binary, workspace.path(), &["react", &root, "ack"]).await?;
    let conflict = phi(binary, workspace.path(), &["react", &root, "ack"])
        .await
        .err()
        .ok_or_else(|| phi_core::Error::User("duplicate ack must fail".into()))?;
    assert!(
        conflict.to_string().contains("conflict: POST /messages/")
            && conflict
                .to_string()
                .contains("ack reaction already exists on message")
    );
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
    let conflict = phi(
        binary,
        workspace.path(),
        &["--as", "observer", "react", &reply, "claim"],
    )
    .await
    .err()
    .ok_or_else(|| phi_core::Error::User("claim conflict must fail".into()))?;
    assert!(
        conflict
            .to_string()
            .contains("already claimed by alice/main just now")
    );
    assert!(
        phi(binary, workspace.path(), &["wait", "0"])
            .await?
            .is_empty()
    );
    let events = phi(binary, workspace.path(), &["--as", "observer", "wait", "0"]).await?;
    assert!(
        events.contains(&format!("thread {thread} · Task"))
            && events.contains("first body")
            && !events.contains("second body")
            && events.contains("your other session")
            && !events.contains("UNTRUSTED")
            && events.contains("ack by alice/main")
            && !events.contains("claimed by alice/main")
            && !events.contains("summary (as of")
            && events.contains(&root)
    );
    assert!(!events.contains("reaction:"));
    assert!(!events.contains("phi read"));
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
    phi(binary, workspace.path(), &["react", &reply, "ack"]).await?;
    phi(
        binary,
        workspace.path(),
        &["space", "set", "summary_every", "4"],
    )
    .await?;
    assert!(
        phi(binary, workspace.path(), &["space", "show"])
            .await?
            .contains("summary_every=4")
    );
    phase.store(1, std::sync::atomic::Ordering::SeqCst);
    let receipt = phi(
        binary,
        workspace.path(),
        &[
            "reply",
            thread,
            "receipt survives lookup failure",
            "--summary",
            "Three messages",
        ],
    )
    .await?;
    assert!(receipt.contains(&format!("thread {thread}")) && receipt.contains(" · id "));
    phase.store(0, std::sync::atomic::Ordering::SeqCst);
    let final_read = phi(binary, workspace.path(), &["read", thread, "--all"]).await?;
    assert!(
        final_read.contains(&format!("message {thread}#3"))
            && final_read.contains("receipt survives lookup failure")
    );
    Ok(())
}

async fn phi(binary: &str, root: &std::path::Path, args: &[&str]) -> phi_core::Result<String> {
    phi_with_input(binary, root, args, None).await
}

async fn phi_with_input(
    binary: &str,
    root: &std::path::Path,
    args: &[&str],
    input: Option<&str>,
) -> phi_core::Result<String> {
    let home = root.join("home");
    std::fs::create_dir_all(&home)?;
    let mut command = tokio::process::Command::new(binary);
    if !args.contains(&"--as") {
        command.args(["--as", "main"]);
    }
    command
        .args(args)
        .current_dir(root)
        .env("HOME", &home)
        .env("USER", "alice")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("appdata"))
        .env("LOCALAPPDATA", root.join("localappdata"))
        .env_remove("PHI_ADMIN_TOKEN")
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(input) = input {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| phi_core::Error::User("missing stdin".into()))?;
        stdin.write_all(input.as_bytes()).await?;
    }
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(phi_core::Error::User(format!(
            "phi {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout).map_err(|e| phi_core::Error::User(e.to_string()))
}
