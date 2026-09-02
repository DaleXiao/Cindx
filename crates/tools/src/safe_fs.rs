//! Symlink-safe filesystem primitives for workspace-managed private artifacts.
//!
//! Managed content (`.cindx/artifacts`, `attachments`, `output-history`,
//! `tool-output`) lives under a *trusted* workspace root that the app resolved
//! when the project was opened. A hostile workspace can pre-plant any of these
//! paths as a symbolic link pointing outside the workspace. Naive
//! `create_dir_all`, `set_permissions`, and truncating opens all *follow*
//! symlinks, so a single normal artifact write could otherwise recursively
//! chmod or overwrite files outside the workspace.
//!
//! Every primitive here validates, from the canonicalized trusted root, that no
//! component between root and target is a symlink (fail-closed), creates real
//! directories, sets modes through `O_NOFOLLOW` descriptors, and publishes files
//! by atomic `rename` (which replaces a destination entry instead of following
//! it). There is deliberately no recursive chmod sweep: each directory whose
//! mode is forced is a real directory this module just created or validated.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

fn symlink_error(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "refusing to operate through a symbolic link: {}",
            path.display()
        ),
    )
}

fn unsafe_component_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "managed path contains an unsafe component (absolute or `..`)",
    )
}

/// Canonicalize `trusted_root` and return `(canonical_root, relative)` after
/// verifying that `target` is lexically under `trusted_root` and that no
/// existing component between them is a symbolic link. Canonicalizing the root
/// once collapses legitimate system symlinks above it (for example macOS
/// `/tmp -> /private/tmp`); attacker-planted symlinks inside the workspace are
/// rejected fail-closed.
fn validate_under_root(trusted_root: &Path, target: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let canonical_root = trusted_root.canonicalize()?;
    let relative = target.strip_prefix(trusted_root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "managed path {} is not under trusted root {}",
                target.display(),
                trusted_root.display()
            ),
        )
    })?;
    let mut current = canonical_root.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(unsafe_component_error());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(symlink_error(&current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok((canonical_root, relative.to_path_buf()))
}

/// Force a mode on a path through an `O_NOFOLLOW` descriptor so a symlink that
/// appears between validation and the chmod is never followed. Best-effort on
/// platforms without unix descriptors.
#[cfg(unix)]
fn set_mode_no_follow(path: &Path, mode: u32, directory: bool) -> io::Result<()> {
    let mut flags = libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if directory {
        flags |= libc::O_DIRECTORY;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)?;
    let result = unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_mode_no_follow(_path: &Path, _mode: u32, _directory: bool) -> io::Result<()> {
    Ok(())
}

fn parent_of(path: &Path) -> io::Result<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "managed path has no parent"))
}

/// Create the managed directory chain under a trusted root with owner-only
/// access, refusing any symlink component. Each directory is created fresh and
/// mode-set through an `O_NOFOLLOW` descriptor; there is no recursive sweep, so
/// a hostile tree beneath a managed root is never traversed or chmod-ed.
pub fn ensure_private_dir(trusted_root: &Path, dir: &Path) -> io::Result<()> {
    let (canonical_root, relative) = validate_under_root(trusted_root, dir)?;
    let mut current = canonical_root;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(unsafe_component_error());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(symlink_error(&current));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("managed path is not a directory: {}", current.display()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error),
        }
        set_mode_no_follow(&current, 0o700, true)?;
    }
    Ok(())
}

/// Atomically write owner-only content under a trusted root. The parent chain is
/// created real, a symlink leaf is rejected, the payload is staged in an
/// `O_EXCL` temporary inside the validated parent, and published by `rename`
/// (which never follows a destination symlink). A post-write canonical
/// containment check guarantees the result is still inside the trusted root.
pub fn write_private_file(trusted_root: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = parent_of(path)?;
    ensure_private_dir(trusted_root, parent)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(symlink_error(path));
        }
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("cindx-private");
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;

    let canonical_root = trusted_root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    if !canonical_path.starts_with(&canonical_root) {
        let _ = fs::remove_file(path);
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed file escaped the trusted root after write",
        ));
    }
    Ok(())
}

/// Create (or truncate) an owner-only streaming file under a trusted root with
/// `O_NOFOLLOW`, refusing a symlink leaf. Used for shell stdout/stderr captures
/// that are written incrementally rather than published atomically.
pub fn create_private_file(trusted_root: &Path, path: &Path) -> io::Result<fs::File> {
    let parent = parent_of(path)?;
    ensure_private_dir(trusted_root, parent)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(symlink_error(path));
        }
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(path)?;
    Ok(file)
}

/// Force owner-only permissions on an existing file under a trusted root,
/// refusing to follow a symlink leaf or any symlink component above it.
pub fn secure_private_file(trusted_root: &Path, path: &Path) -> io::Result<()> {
    validate_under_root(trusted_root, path)?;
    set_mode_no_follow(path, 0o600, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn ensures_a_real_private_directory_chain() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".cindx").join("artifacts");
        ensure_private_dir(root.path(), &dir).unwrap();
        assert!(dir.is_dir());
        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&root.path().join(".cindx")), 0o700);
    }

    #[test]
    fn rejects_a_symlinked_managed_root_and_leaves_target_untouched() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("keep.sh");
        fs::write(&outside_file, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&outside_file, fs::Permissions::from_mode(0o755)).unwrap();
        let before = fs::metadata(&outside_file).unwrap();

        // Hostile workspace pre-plants `.cindx/artifacts` as a link outside.
        fs::create_dir_all(root.path().join(".cindx")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".cindx/artifacts")).unwrap();

        let dir = root.path().join(".cindx").join("artifacts");
        let result = ensure_private_dir(root.path(), &dir);
        assert!(result.is_err(), "symlinked managed root must fail closed");

        // Zero external change: content, mode, and mtime are intact.
        assert_eq!(fs::read(&outside_file).unwrap(), b"#!/bin/sh\n");
        assert_eq!(mode_of(&outside_file), 0o755);
        assert_eq!(
            fs::metadata(&outside_file).unwrap().modified().unwrap(),
            before.modified().unwrap()
        );
    }

    #[test]
    fn rejects_a_symlinked_ancestor_component() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".cindx")).unwrap();
        let dir = root.path().join(".cindx").join("attachments");
        assert!(ensure_private_dir(root.path(), &dir).is_err());
        // The outside directory was never turned into a 0700 managed dir tree.
        assert!(!outside.path().join("attachments").exists());
    }

    #[test]
    fn write_rejects_a_symlink_leaf_and_never_touches_the_target() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, b"original-secret").unwrap();
        fs::set_permissions(&outside_file, fs::Permissions::from_mode(0o644)).unwrap();

        let dir = root.path().join(".cindx").join("artifacts");
        ensure_private_dir(root.path(), &dir).unwrap();
        let leaf = dir.join("image.png");
        std::os::unix::fs::symlink(&outside_file, &leaf).unwrap();

        let result = write_private_file(root.path(), &leaf, b"attacker-payload");
        assert!(result.is_err(), "symlink leaf must fail closed");
        // The external file is byte-for-byte and mode-for-mode unchanged.
        assert_eq!(fs::read(&outside_file).unwrap(), b"original-secret");
        assert_eq!(mode_of(&outside_file), 0o644);
    }

    #[test]
    fn write_publishes_atomically_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join(".cindx")
            .join("artifacts")
            .join("out.json");
        write_private_file(root.path(), &path, b"{\"ok\":true}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"ok\":true}");
        assert_eq!(mode_of(&path), 0o600);
        assert!(path
            .canonicalize()
            .unwrap()
            .starts_with(root.path().canonicalize().unwrap()));
    }

    #[test]
    fn create_private_file_rejects_a_symlink_leaf() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("stdout.log");
        fs::write(&outside_file, b"keep").unwrap();
        let dir = root.path().join(".cindx").join("tool-output");
        ensure_private_dir(root.path(), &dir).unwrap();
        let leaf = dir.join("stdout.log");
        std::os::unix::fs::symlink(&outside_file, &leaf).unwrap();
        assert!(create_private_file(root.path(), &leaf).is_err());
        assert_eq!(fs::read(&outside_file).unwrap(), b"keep");
    }

    #[test]
    fn rejects_a_target_outside_the_trusted_root() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let target = elsewhere.path().join("escape");
        assert!(ensure_private_dir(root.path(), &target).is_err());
    }
}
