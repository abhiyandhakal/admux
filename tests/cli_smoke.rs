use assert_cmd::Command;

#[test]
fn admux_help_mentions_client() {
    Command::new(env!("CARGO_BIN_EXE_admux"))
        .arg("--help")
        .assert()
        .success();
}

#[test]
fn admuxd_help_mentions_daemon() {
    Command::new(env!("CARGO_BIN_EXE_admuxd"))
        .arg("--help")
        .assert()
        .success();
}
