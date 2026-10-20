use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use phi_server::{
    Server,
    config::{Config, TlsMode},
};
use std::path::PathBuf;

mod init;

#[derive(Parser)]
#[command(version, about = "Encrypted agent messaging relay")]
struct Cli {
    #[arg(
        long,
        env = "PHI_SERVER_CONFIG",
        default_value = "server.toml",
        global = true
    )]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Prepare a reverse-proxy deployment with a private admin token.
    Init {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    Backup {
        dest: PathBuf,
    },
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "phi_server=info".into()),
        )
        .init();
    let cli = Cli::parse();
    if let Some(Command::Init { dir }) = &cli.command {
        return init::run(dir);
    }
    let config = Config::load(&cli.config)?;
    let server = Server::open(config.clone())?;
    if let Some(Command::Backup { dest }) = cli.command {
        return server.backup(dest).await;
    }
    let retention = server.start_retention();
    tracing::info!(listen=%config.listen,"starting phi-server");
    let router = server.router();
    match config.tls.mode {
        TlsMode::Off => {
            let listener = tokio::net::TcpListener::bind(config.listen).await?;
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown())
                .await?;
        }
        TlsMode::Acme => {
            let mut state = rustls_acme::AcmeConfig::new([config
                .tls
                .domain
                .ok_or_else(|| anyhow::anyhow!("missing domain"))?])
            .contact([format!(
                "mailto:{}",
                config
                    .tls
                    .email
                    .ok_or_else(|| anyhow::anyhow!("missing email"))?
            )])
            .cache(rustls_acme::caches::DirCache::new(
                config.data_dir.join("acme"),
            ))
            .directory_lets_encrypt(true)
            .state();
            let acceptor = state.axum_acceptor(state.default_rustls_config());
            let acme = tokio::spawn(async move {
                while let Some(event) = state.next().await {
                    match event {
                        Ok(event) => tracing::info!(?event, "ACME event"),
                        Err(error) => tracing::error!(?error, "ACME error"),
                    }
                }
            });
            let handle = axum_server::Handle::new();
            let stop = handle.clone();
            tokio::spawn(async move {
                shutdown().await;
                stop.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
            });
            axum_server::bind(config.listen)
                .handle(handle)
                .acceptor(acceptor)
                .serve(router.into_make_service())
                .await?;
            acme.abort();
        }
    }
    retention.abort();
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
