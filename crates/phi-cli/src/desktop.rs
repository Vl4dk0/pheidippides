use std::{
    ffi::OsStr,
    path::PathBuf,
    process::{Command, Stdio},
};

pub(crate) struct Launcher {
    pub program: PathBuf,
    pub args: Vec<&'static str>,
}

// Keep discovery separate from execution so tests never open a window.
pub(crate) fn candidates(platform: &str, path: &OsStr) -> Vec<Launcher> {
    let mut candidates = Vec::new();
    if platform == "macos" {
        candidates.push(Launcher {
            program: "/usr/bin/open".into(),
            args: vec!["-a", "Pheidippides"],
        });
    }
    let names: &[&str] = if platform == "windows" {
        &["phi-gui.exe", "Pheidippides.exe"]
    } else {
        &["phi-gui", "Pheidippides"]
    };
    for directory in std::env::split_paths(path) {
        for name in names {
            let program = directory.join(name);
            if program.is_file() {
                candidates.push(Launcher {
                    program,
                    args: vec![],
                });
            }
        }
    }
    candidates
}

pub(crate) fn open_installed() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for launcher in candidates(std::env::consts::OS, &path) {
        let mut command = Command::new(launcher.program);
        command
            .args(&launcher.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if launcher.args.is_empty() {
            if command.spawn().is_ok() {
                return true;
            }
        } else if command.status().is_ok_and(|status| status.success()) {
            return true;
        }
    }
    false
}
