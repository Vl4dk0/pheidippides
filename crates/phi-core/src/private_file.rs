//! Private key, draft and deployment files.
//!
//! Unix mode 0600/0700 is applied at creation and re-applied here, because a
//! home directory is often world-readable and mode bits do not inherit.
//!
//! Windows needs no extra step for profile and cache files: they live under
//! the `directories` crate roots (`%APPDATA%\pheidippides`,
//! `%LOCALAPPDATA%\pheidippides`), and new files there inherit the per-user
//! profile ACL that admits only the user, SYSTEM and Administrators — the same
//! boundary as root reading a 0600 file. Rewriting that DACL would need
//! either `unsafe` Win32 calls (forbidden in this workspace) or a helper
//! process, whose failure would make storage unusable for no added
//! protection. Files placed in operator-chosen directories (`phi-server
//! init`'s `.env`) do need an explicit DACL; init sets that itself.
//!
//! Directory junctions and other reparse points are refused alongside
//! symlinks so a storage path cannot point somewhere else.

use std::{fs, path::Path};

pub fn is_link(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    meta.file_type().is_symlink() || is_reparse_point(&meta)
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn is_reparse_point(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions are reparse points, not Rust symlinks. OneDrive
        // placeholders are too; following either would write secrets elsewhere.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

pub fn restrict_owner(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(windows)]
    let _ = path;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        compile_error!("private files need unix modes or a per-user windows profile");
    }
    Ok(())
}
