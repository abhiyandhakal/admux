use admux::test_support::wait_for_path;
use assert_cmd::Command;
use predicates::prelude::*;
use std::{
    path::Path,
    process::{Child, Command as StdCommand, Stdio},
    thread,
    time::{Duration, Instant},
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
        .args([
            "new",
            "-d",
            "--name",
            "work",
            "--",
            "sh",
            "-lc",
            "printf work-ready; sleep 1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("created work pane"));

    Command::new(env!("CARGO_BIN_EXE_admux"))
        .env("ADMUX_SOCKET", &socket)
        .env("ADMUX_CONFIG", &config)
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("work"));

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_output = false;
    while Instant::now() < deadline {
        let output = StdCommand::new(env!("CARGO_BIN_EXE_admux"))
            .env("ADMUX_SOCKET", &socket)
            .env("ADMUX_CONFIG", &config)
            .args(["attach", "work"])
            .output()
            .expect("run attach");
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success()
            && stdout.contains("attached work")
            && stdout.contains("work-ready")
        {
            saw_output = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(saw_output, "attach never exposed pane output");

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
