use admux::test_support::wait_for_path;
use assert_cmd::Command;
use predicates::prelude::*;
use std::{
    path::Path,
    process::{Child, Command as StdCommand, Stdio},
    time::Duration,
};
use tempfile::tempdir;

fn spawn_daemon(socket: &Path) -> Child {
    let child = StdCommand::new(env!("CARGO_BIN_EXE_admuxd"))
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn admuxd");
    assert!(wait_for_path(socket, Duration::from_secs(2)));
    child
}

#[test]
fn daemon_backed_cli_can_manage_sessions() {
    let temp = tempdir().expect("tempdir");
    let socket = temp.path().join("runtime").join("admux.sock");
    let config = temp.path().join("config.toml");
    std::fs::write(&config, "").expect("write config");
    let mut daemon = spawn_daemon(&socket);

    Command::new(env!("CARGO_BIN_EXE_admux"))
        .env("ADMUX_SOCKET", &socket)
        .env("ADMUX_CONFIG", &config)
        .args(["new", "--name", "work"])
        .assert()
        .success()
        .stdout(predicate::str::contains("created work"));

    Command::new(env!("CARGO_BIN_EXE_admux"))
        .env("ADMUX_SOCKET", &socket)
        .env("ADMUX_CONFIG", &config)
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("work"));

    Command::new(env!("CARGO_BIN_EXE_admux"))
        .env("ADMUX_SOCKET", &socket)
        .env("ADMUX_CONFIG", &config)
        .args(["attach", "work"])
        .assert()
        .success()
        .stdout(predicate::str::contains("attached work"));

    Command::new(env!("CARGO_BIN_EXE_admux"))
        .env("ADMUX_SOCKET", &socket)
        .env("ADMUX_CONFIG", &config)
        .args(["kill", "work"])
        .assert()
        .success()
        .stdout(predicate::str::contains("killed work"));

    let _ = daemon.kill();
    let _ = daemon.wait();
}
