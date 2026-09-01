//! Unix permission hardening for workspace-hosted managed artifacts.
//!
//! Attachments, tool-result artifacts (including MCP images), shell stream
//! captures, and knowledge generations are user-private content stored under
//! `.cindx`; they must never default to world-readable (0644) on a shared
//! machine. Writes go through a 0600 open (plus a permission force for
//! pre-existing files), directories through 0700, and every ensure call also
//! performs a bounded best-effort sweep so legacy 0644 files migrate on the
//! next managed write instead of persisting forever.

use std::fs;
use std::io;
use std::path::Path;

#[cfg(unix)]
fn mode_permissions(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}

/// Create (or truncate) a file that only the owner may read or write. A
/// pre-existing file keeps no world-readable mode: the permissions are forced
/// after the open.
pub(crate) fn private_file_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        fs::set_permissions(path, mode_permissions(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)
    }
}

/// Force owner-only permissions on a pre-existing file (for example one
/// created by `fs::copy`, which inherits the source's mode). No-op where the
/// platform has no unix permission bits.
pub(crate) fn private_file_secure(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, mode_permissions(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Create the directory chain for a managed artifact directory and force the
/// leaf to owner-only access, then sweep any legacy entries beneath it.
pub(crate) fn private_dir_ensure(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, mode_permissions(0o700))?;
    enforce_private_tree(path);
    Ok(())
}

/// Best-effort migration: directories become 0700 and files 0600 beneath a
/// managed root. Bounded by depth and entry count so a hostile or enormous
/// tree cannot stall a write path; failures are ignored by design (the
/// primary write still enforces its own mode).
pub(crate) fn enforce_private_tree(root: &Path) {
    #[cfg(unix)]
    {
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        let mut visited = 0usize;
        while let Some((dir, depth)) = stack.pop() {
            if visited > 10_000 || depth > 8 {
                return;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                visited += 1;
                if visited > 10_000 {
                    return;
                }
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    let _ = fs::set_permissions(&path, mode_permissions(0o700));
                    stack.push((path, depth + 1));
                } else if file_type.is_file() {
                    let _ = fs::set_permissions(&path, mode_permissions(0o600));
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
}
