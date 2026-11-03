#![allow(clippy::unwrap_used)]
//! Security audit of 2026-11-02, stage 1: fails until stage 2 (one function opens files of the
//! copy, and it refuses what is not a regular file).

use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use clio_node::Sandbox;

// Finding 3 (attack-50), S-13: a program that ran in the jail can leave a FIFO in the folder.
// The snapshot copies every file; copying a FIFO waits for a writer forever.
#[test]
fn a_snapshot_does_not_hang_on_a_fifo() {
    let t = tempfile::tempdir().unwrap();
    let (root, scratch) = (t.path().join("work"), t.path().join("scratch"));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(root.join("f"))
            .status()
            .unwrap()
            .success()
    );
    let mut sb = Sandbox::new(&root, &scratch).unwrap();
    sb.set_copy_snapshots(true); // what the host sets once a program can run in the folder
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(sb.snapshot().map(|s| s.discard()).is_ok());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "snapshot is stuck on a FIFO"
    );
}
