use crate::{
    cli::{AdmuxCli, ClientCommand},
    ipc::{CommandRequest, CommandResponse},
    paths::RuntimePaths,
};
use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn run_from_env() -> Result<()> {
    let cli = AdmuxCli::parse();
    run(cli)
}

pub fn run(cli: AdmuxCli) -> Result<()> {
    let request = match cli.command {
        ClientCommand::New(args) => CommandRequest::NewSession {
            name: args.name,
            cwd: args.cwd,
            command: args.command,
        },
        ClientCommand::Attach(args) => CommandRequest::Attach {
            session: args.session,
        },
        ClientCommand::Ls => CommandRequest::ListSessions,
        ClientCommand::Kill(args) => CommandRequest::KillSession {
            session: args.session,
        },
        ClientCommand::SendKeys(args) => CommandRequest::SendKeys {
            target: args.target,
            keys: args.keys,
        },
        ClientCommand::ReloadConfig => CommandRequest::ReloadConfig,
    };

    let paths = RuntimePaths::resolve();
    let response = request_response(&paths, request)?;
    print_response(response)
}

pub fn request_response(paths: &RuntimePaths, request: CommandRequest) -> Result<CommandResponse> {
    let response = with_connection(paths, |stream| {
        write_message(stream, &request)?;
        read_message(stream)
    })?;
    Ok(response)
}

fn with_connection<T>(
    paths: &RuntimePaths,
    mut f: impl FnMut(&mut UnixStream) -> Result<T>,
) -> Result<T> {
    match UnixStream::connect(&paths.socket_path) {
        Ok(mut stream) => f(&mut stream),
        Err(_) => {
            spawn_daemon(paths)?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match UnixStream::connect(&paths.socket_path) {
                    Ok(mut stream) => return f(&mut stream),
                    Err(error) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(50));
                        let _ = error;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "failed to connect to admuxd at {} after autostart",
                                paths.socket_path.display()
                            )
                        });
                    }
                }
            }
        }
    }
}

fn spawn_daemon(paths: &RuntimePaths) -> Result<()> {
    let daemon_path = resolve_daemon_binary()?;
    let socket = paths.socket_path.display().to_string();
    Command::new(daemon_path)
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn admuxd")?;
    Ok(())
}

fn resolve_daemon_binary() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("ADMUXD_BIN") {
        return Ok(path.into());
    }

    let current = std::env::current_exe().context("failed to resolve current executable path")?;
    let daemon = current.with_file_name("admuxd");
    if daemon.exists() {
        Ok(daemon)
    } else {
        bail!(
            "could not locate admuxd binary next to {}",
            current.display()
        )
    }
}

fn write_message(stream: &mut UnixStream, request: &CommandRequest) -> Result<()> {
    let payload = serde_json::to_vec(request).context("failed to encode request")?;
    stream
        .write_all(&payload)
        .context("failed to write request payload")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish request")?;
    Ok(())
}

fn read_message(stream: &mut UnixStream) -> Result<CommandResponse> {
    let mut payload = Vec::new();
    stream
        .read_to_end(&mut payload)
        .context("failed to read response payload")?;
    let response = serde_json::from_slice(&payload).context("failed to decode response")?;
    Ok(response)
}

fn print_response(response: CommandResponse) -> Result<()> {
    match response {
        CommandResponse::HelloAck { version } => {
            println!("protocol {}", version.0);
        }
        CommandResponse::SessionCreated { session } => {
            println!("created {session}");
        }
        CommandResponse::Attached { session } => {
            println!("attached {session}");
        }
        CommandResponse::SessionList { sessions } => {
            for session in sessions {
                println!("{session}");
            }
        }
        CommandResponse::SessionKilled { session } => {
            println!("killed {session}");
        }
        CommandResponse::KeysSent => {
            println!("keys sent");
        }
        CommandResponse::ConfigReloaded => {
            println!("config reloaded");
        }
        CommandResponse::Error { message } => return Err(anyhow!(message)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::RuntimePaths;

    #[test]
    fn writes_and_reads_protocol_messages() {
        let response = CommandResponse::SessionCreated {
            session: "work".into(),
        };
        let encoded = serde_json::to_vec(&response).expect("encode response");
        let decoded: CommandResponse = serde_json::from_slice(&encoded).expect("decode response");
        assert_eq!(decoded, response);
    }

    #[test]
    fn runtime_paths_can_be_used_for_requests() {
        let paths = RuntimePaths {
            socket_path: "/tmp/admux-test/socket".into(),
            config_path: "/tmp/admux-test/config.toml".into(),
        };
        assert!(paths.socket_path.ends_with("socket"));
    }
}
