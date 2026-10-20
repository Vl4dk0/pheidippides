use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(crate) struct Launcher {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Install locations plain `phi` may open. Tests pass temporary roots so
/// discovery never depends on what this machine has installed.
pub(crate) struct Roots {
    pub local_app_data: Option<PathBuf>,
    pub program_files: Option<PathBuf>,
    pub application_dirs: Vec<PathBuf>,
    pub system_bin: Option<PathBuf>,
}

// Keep discovery separate from execution so tests never open a window.
// Names match the Tauri bundle: product "Pheidippides", binary "pheidippides".
// Windows NSIS (Tauri 2.11, install mode currentUser) uses
// %LOCALAPPDATA%\Pheidippides. %LOCALAPPDATA%\Programs is the per-user
// convention some installers use instead. perMachine NSIS and WiX/MSI use
// Program Files. Linux .deb installs /usr/bin/pheidippides; an AppImage has
// no install directory, so only ~/Applications (and PATH) is tried.
pub(crate) fn candidates(platform: &str, path: &OsStr, roots: &Roots) -> Vec<Launcher> {
    let mut found = Vec::new();
    if platform == "macos" {
        found.push(Launcher {
            program: PathBuf::from("/usr/bin/open"),
            args: vec!["-a".into(), "Pheidippides".into()],
        });
        for dir in &roots.application_dirs {
            let app = dir.join("Pheidippides.app");
            if app.is_dir() {
                found.push(Launcher {
                    program: PathBuf::from("/usr/bin/open"),
                    args: vec![app.to_string_lossy().into_owned()],
                });
            }
        }
    }
    if platform == "windows" {
        for program in windows_install_dirs(roots) {
            push_file(&mut found, program);
        }
    }
    let names: &[&str] = if platform == "windows" {
        &["pheidippides.exe"]
    } else {
        &["pheidippides"]
    };
    for directory in std::env::split_paths(path) {
        for name in names {
            push_file(&mut found, directory.join(name));
        }
    }
    if platform == "linux" {
        if let Some(bin) = &roots.system_bin {
            push_file(&mut found, bin.join("pheidippides"));
        }
        for dir in &roots.application_dirs {
            push_file(&mut found, dir.join("Pheidippides.AppImage"));
            push_file(&mut found, dir.join("pheidippides.AppImage"));
        }
    }
    found
}

fn windows_install_dirs(roots: &Roots) -> Vec<PathBuf> {
    let mut programs = Vec::new();
    let nested = Path::new("Pheidippides").join("pheidippides.exe");
    if let Some(local) = &roots.local_app_data {
        programs.push(local.join(&nested));
        programs.push(local.join("Programs").join(&nested));
    }
    if let Some(program_files) = &roots.program_files {
        programs.push(program_files.join(&nested));
    }
    programs
}

fn push_file(found: &mut Vec<Launcher>, program: PathBuf) {
    if program.is_file() && found.iter().all(|launcher| launcher.program != program) {
        found.push(Launcher {
            program,
            args: Vec::new(),
        });
    }
}

pub(crate) fn open_installed() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for launcher in candidates(std::env::consts::OS, &path, &host_roots()) {
        let mut command = Command::new(&launcher.program);
        command
            .args(&launcher.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = if launcher.args.is_empty() {
            command.spawn().is_ok()
        } else {
            command.status().is_ok_and(|status| status.success())
        };
        if started {
            return true;
        }
    }
    false
}

fn host_roots() -> Roots {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let mut application_dirs = Vec::new();
    if cfg!(target_os = "macos") {
        application_dirs.push(PathBuf::from("/Applications"));
    }
    if let Some(home) = &home {
        application_dirs.push(home.join("Applications"));
    }
    Roots {
        local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        program_files: std::env::var_os("ProgramFiles").map(PathBuf::from),
        application_dirs,
        system_bin: if cfg!(unix) {
            Some(PathBuf::from("/usr/bin"))
        } else {
            None
        },
    }
}
