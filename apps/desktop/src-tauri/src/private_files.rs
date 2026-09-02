//! Unix permission hardening for workspace-hosted managed artifacts.
//!
//! Attachments, tool-result artifacts (including MCP images), shell stream
//! captures, and knowledge generations are user-private content stored under
//! `.cindx`; they must never default to world-readable (0644) on a shared
//! machine, and a hostile workspace must not be able to redirect these writes
//! outside the project through a planted symbolic link.
//!
//! All operations delegate to the symlink-safe primitives in `tools::safe_fs`,
//! which validate every path component against the trusted workspace root,
//! force owner-only modes through `O_NOFOLLOW` descriptors, and publish files
//! atomically. There is deliberately no recursive chmod sweep beneath a managed
//! root: a hostile tree (or a symlinked root) is never traversed. Legacy 0644
//! migration is handled by a separate, allowlisted startup migrator rather than
//! on every write.

use std::io;
use std::path::Path;

/// Create the directory chain for a managed artifact directory under the
/// trusted workspace root and force the leaf to owner-only access, refusing any
/// symbolic-link component between the root and the target.
pub(crate) fn private_dir_ensure(trusted_root: &Path, path: &Path) -> io::Result<()> {
    tools::ensure_private_dir(trusted_root, path)
}

/// Atomically write owner-only content under the trusted workspace root,
/// refusing to follow a symlink leaf and verifying the published file stays
/// inside the root.
pub(crate) fn private_file_write(trusted_root: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    tools::write_private_file(trusted_root, path, bytes)
}

/// Force owner-only permissions on a pre-existing file under the trusted root
/// (for example one created by `fs::copy`, which inherits the source's mode),
/// refusing to follow a symlink leaf or any symlink component above it.
pub(crate) fn private_file_secure(trusted_root: &Path, path: &Path) -> io::Result<()> {
    tools::secure_private_file(trusted_root, path)
}
