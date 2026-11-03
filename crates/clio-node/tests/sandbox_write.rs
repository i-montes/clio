#![allow(clippy::unwrap_used)]
//! `Sandbox::write` replaces a file (write, then rename) so a hard-linked snapshot never changes.
//! The new file must not lose what the old one had.

use std::os::unix::fs::PermissionsExt;

use clio_node::Sandbox;

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

// An agent that edits a script must not make it stop being executable.
#[test]
fn writing_over_an_executable_keeps_it_executable() {
    let t = tempfile::tempdir().unwrap();
    let (root, scratch) = (t.path().join("work"), t.path().join("scratch"));
    std::fs::create_dir(&root).unwrap();
    let sb = Sandbox::new(&root, &scratch).unwrap();
    sb.write("run.sh", b"#!/bin/sh\necho one\n").unwrap();
    std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    sb.write("run.sh", b"#!/bin/sh\necho two\n").unwrap();
    assert_eq!(mode(&root.join("run.sh")), 0o755);
    assert_eq!(
        std::fs::read(root.join("run.sh")).unwrap(),
        b"#!/bin/sh\necho two\n"
    );
    // A new file is an ordinary one: nothing makes it executable.
    sb.write("notes.txt", b"x").unwrap();
    assert_eq!(mode(&root.join("notes.txt")) & 0o111, 0);
}
