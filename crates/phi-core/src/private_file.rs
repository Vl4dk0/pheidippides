//! Private key, draft and deployment files.
//!
//! Unix mode 0600/0700 is applied at creation. Windows has no mode bits, and
//! this workspace forbids `unsafe`, so Win32 ACL calls are not available.
//! Instead, after each private create we set an owner-only DACL through
//! PowerShell's .NET ACL types (present on every Windows that can run the
//! GUI). The current user's SID gets full control; inheritance is disabled
//! and no other identity is granted access. Administrators can still take
//! ownership, which is the same boundary as root reading a 0600 file.
//!
//! Profile and cache files also live under the `directories` crate roots
//! (`%APPDATA%\pheidippides`, `%LOCALAPPDATA%\pheidippides`). Those profile
//! folders already admit only the user, SYSTEM and Administrators. The
//! explicit DACL still matters: `phi-server init` writes `.env` into an
//! operator-chosen directory whose inherited ACL may be readable by every
//! local user. Directory junctions and other reparse points are refused
//! alongside symlinks so a storage path cannot point somewhere else.

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
    restrict_owner_dacl(path)?;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        compile_error!("private files need unix modes or a windows ACL");
    }
    Ok(())
}

#[cfg(windows)]
fn restrict_owner_dacl(path: &Path) -> std::io::Result<()> {
    // The path travels in the environment, not in the script text, so a
    // storage path with quotes or spaces cannot change the ACL command.
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$path = $env:PHI_PRIVATE_PATH
if ([string]::IsNullOrWhiteSpace($path)) { throw 'PHI_PRIVATE_PATH is empty' }
$item = Get-Item -LiteralPath $path -Force
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
if ($null -eq $sid) { throw 'current process has no user SID' }
$inherit = [System.Security.AccessControl.InheritanceFlags]::None
if ($item.PSIsContainer) {
  $inherit = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'
}
$rule = New-Object System.Security.AccessControl.FileSystemAccessRule(
  $sid, 'FullControl', $inherit,
  [System.Security.AccessControl.PropagationFlags]::None, 'Allow')
if ($item.PSIsContainer) {
  $acl = New-Object System.Security.AccessControl.DirectorySecurity
} else {
  $acl = New-Object System.Security.AccessControl.FileSecurity
}
$acl.SetAccessRuleProtection($true, $false)
[void]$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $path -AclObject $acl
"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("PHI_PRIVATE_PATH", path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| {
            std::io::Error::other(format!(
                "set owner-only ACL via PowerShell for {}: {error}",
                path.display()
            ))
        })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(std::io::Error::other(format!(
        "owner-only ACL failed for {}: {}",
        path.display(),
        detail.trim()
    )))
}
