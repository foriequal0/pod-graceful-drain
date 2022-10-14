use std::fmt::{Display, Formatter, Write as _};
use std::io::{stderr, stdout, BufRead, BufReader, Write};
use std::process::{Command, Output, Stdio};

use eyre::{eyre, Context, Result};
use tokio::task::spawn_blocking;
use tracing::{enabled, error, info, span, trace, Level};

#[derive(Clone)]
pub struct CommandParams<'a> {
    pub command: &'a str,
    pub config_args: &'a [&'a str],
    pub args: &'a [&'a str],
    pub stdin: Option<Vec<u8>>,
}

impl Display for CommandParams<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        append(self.command, f)?;
        for arg in self.args {
            f.write_char(' ')?;
            append(arg, f)?;
        }
        if let Some(stdin) = self.stdin.as_ref() {
            if let Ok(str) = std::str::from_utf8(stdin) {
                f.write_str(" <<EOF\n")?;
                f.write_str(str)?;
                f.write_str("EOF")?;
            }
        }

        return Ok(());

        fn append(str: &str, f: &mut Formatter<'_>) -> std::fmt::Result {
            if str.contains([' ', '\'']) {
                f.write_char('\'')?;
                for c in str.chars() {
                    if c == '\'' {
                        f.write_str("'\''")?;
                    }
                }
                f.write_char('\'')?;
            } else {
                f.write_str(str)?;
            }

            Ok(())
        }
    }
}

pub async fn run_command(params: &CommandParams<'_>) -> Result<()> {
    assert!(
        enabled!(target: "test", Level::INFO),
        "logger should be set"
    );

    let mut command = Command::new(params.command);
    command.args(params.config_args);
    command.args(params.args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if params.stdin.is_some() {
        command.stdin(Stdio::piped());
    }

    let mut child = spawn_blocking(move || command.spawn()).await??;

    let span = span!(target: "test", Level::ERROR, "command", pid=child.id());

    info!(target: "test", parent: &span, "{}", params);

    if let Some(stdin) = params.stdin.as_ref() {
        let mut pipe = child.stdin.as_ref().expect("piped");
        pipe.write_all(stdin)?;
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_blocking({
        let stdout = child.stdout.take().expect("piped");
        let tx = tx.clone();
        move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(("stdout", line.unwrap())).is_err() {
                    break;
                }
            }
        }
    });
    spawn_blocking({
        let stderr = child.stderr.take().expect("piped");
        let tx = tx.clone();
        move || {
            for line in BufReader::new(stderr).lines() {
                if tx.send(("stderr", line.unwrap())).is_err() {
                    break;
                }
            }
        }
    });

    // logger is installed on this thread, so we send logs to this thread.
    tokio::spawn({
        let span = span.clone();
        async move {
            while let Some((stream, line)) = rx.recv().await {
                trace!(target: "test", parent: &span, stream=stream, "{}", line);
            }
        }
    });

    let output = spawn_blocking(move || child.wait_with_output()).await??;

    if output.status.success() {
        trace!(target: "test", parent: &span, exit_code=output.status.code(), "exited");
    } else {
        error!(target: "test", parent: &span, exit_code=output.status.code(), "exited");
    };

    if !output.status.success() {
        return Err(eyre!("Command({params}) ({:?}):", output.status));
    }

    Ok(())
}

pub async fn get_command_output(params: &CommandParams<'_>) -> Result<Vec<u8>> {
    let span = span!(target: "test", Level::ERROR, "command", %params);
    assert!(!span.is_disabled(), "logger should be set");

    info!(target: "test", parent: &span, "start");

    let mut command = Command::new(params.command);
    command.args(params.config_args);
    command.args(params.args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if params.stdin.is_some() {
        command.stdin(Stdio::piped());
    }

    let output = spawn_blocking({
        let params = params.clone();
        move || -> Result<Output> {
            let child = command.spawn()?;

            if let Some(stdin) = params.stdin.as_ref() {
                let mut pipe = child.stdin.as_ref().expect("piped");
                pipe.write_all(stdin)?;
            }

            Ok(child.wait_with_output()?)
        }
    })
    .await
    .context(format!("command: {params}"))??;

    info!(target: "test", parent: &span, "exited ({status})", status = output.status);

    if !output.status.success() {
        stdout().write_all(&output.stdout).unwrap();
        stderr().write_all(&output.stderr).unwrap();
        return Err(eyre!("Command({params}) ({:?}):", output.status));
    }

    Ok(output.stdout)
}
