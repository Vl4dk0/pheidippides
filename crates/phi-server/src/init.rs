use anyhow::Context;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

pub(super) fn run(dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dir).context("create deployment directory")?;
    let config_path = dir.join("server.toml");
    let env_path = dir.join(".env");
    for path in [&config_path, &env_path] {
        match fs::symlink_metadata(path) {
            Ok(_) => anyhow::bail!("refusing to overwrite {}", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context(format!("inspect {}", path.display())),
        }
    }
    let mut random = [0; 32];
    getrandom::fill(&mut random)
        .map_err(|error| anyhow::anyhow!("generate admin token: {error}"))?;
    let token = serde_json::to_value(phi_proto::Base64::from_bytes(random))?;
    let token = token
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("admin token encoding failed"))?;

    // Exclusive creation also protects against races after the preflight check.
    let mut config = create(&config_path, false)?;
    let mut env = match create(&env_path, true) {
        Ok(file) => file,
        Err(error) => {
            fs::remove_file(&config_path).context("remove unused config after init failure")?;
            return Err(error);
        }
    };
    let result = (|| -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Only the secret stays private; container UIDs must be able to read config.
            config.set_permissions(fs::Permissions::from_mode(0o644))?;
            env.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        config.write_all(include_bytes!("../../../deploy/server.toml"))?;
        writeln!(
            env,
            "# Keep private: Compose or your shell exports this to phi-server.\nPHI_SERVER_ADMIN_TOKEN={token}"
        )?;
        config.sync_all()?;
        env.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        fs::remove_file(&env_path).context("remove incomplete token file")?;
        fs::remove_file(&config_path).context("remove incomplete config")?;
        return Err(error).context("write deployment files");
    }

    println!(
        "Created {} and {} (private admin token).\n\
         Next steps (from the deployment directory):\n\
         1. Load the token: set -a; . ./.env; set +a\n\
         2. Run: phi-server --config server.toml\n\
         3. Create a space (in another shell with .env loaded):\n\
            PHI_ADMIN_TOKEN=\"$PHI_SERVER_ADMIN_TOKEN\" phi space create game --server http://127.0.0.1:8443\n\
         Use your public HTTPS URL when behind a reverse proxy.\n\
         For Docker Compose and TLS setup, see deploy/README.md.",
        config_path.display(),
        env_path.display()
    );
    Ok(())
}

fn create(path: &Path, private: bool) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = private;
    options.open(path).with_context(|| {
        format!(
            "create {} (existing files are never overwritten)",
            path.display()
        )
    })
}
