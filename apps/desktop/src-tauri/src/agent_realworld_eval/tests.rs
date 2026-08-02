use super::{directory_size, selected_replicates};
use std::fs;

#[cfg(unix)]
#[test]
fn workspace_size_does_not_follow_external_symlinks() {
    use std::os::unix::fs::symlink;

    let external = tempfile::tempdir().expect("external tempdir");
    fs::write(external.path().join("large.bin"), vec![0_u8; 64 * 1024])
        .expect("external fixture");
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    fs::write(workspace.path().join("local.txt"), b"local").expect("local fixture");
    symlink(external.path(), workspace.path().join("shared-runtime")).expect("runtime symlink");

    assert_eq!(directory_size(workspace.path()), 5);
}

#[test]
fn replicate_selection_is_bounded() {
    std::env::set_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX", "2");
    assert_eq!(selected_replicates(3).expect("selection"), vec![2]);
    std::env::set_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX", "4");
    assert!(selected_replicates(3).is_err());
    std::env::remove_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX");
}
