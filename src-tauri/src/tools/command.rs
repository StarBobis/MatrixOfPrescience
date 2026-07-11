use serde_json::Value;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use crate::tools::execution::{
    resolve_workspace_relative_path, tool_arg_string, tool_arg_string_array, tool_arg_string_map,
    tool_arg_usize, trace_step_with_detail, ToolStreamSink,
};

const COMMAND_STREAM_CHUNK_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug)]
enum CommandOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
enum CommandOutputEvent {
    Chunk(CommandOutputStream, String),
    ReadError(CommandOutputStream, String),
    Done(CommandOutputStream),
}

impl CommandOutputStream {
    fn label(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

fn spawn_command_output_reader<R: Read + Send + 'static>(
    mut reader: R,
    stream: CommandOutputStream,
    sender: Sender<CommandOutputEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; COMMAND_STREAM_CHUNK_SIZE];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read_len) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read_len]).to_string();

                    if sender
                        .send(CommandOutputEvent::Chunk(stream, chunk))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(CommandOutputEvent::ReadError(stream, error.to_string()));
                    break;
                }
            }
        }

        let _ = sender.send(CommandOutputEvent::Done(stream));
    })
}

fn emit_command_output_chunk(
    stream_sink: &mut Option<&mut ToolStreamSink<'_>>,
    stream: CommandOutputStream,
    chunk: &str,
) {
    let Some(sink) = stream_sink.as_deref_mut() else {
        return;
    };

    if chunk.is_empty() {
        return;
    }

    sink(trace_step_with_detail(
        "tool",
        chunk.to_string(),
        stream.label().to_string(),
    ));
}

fn handle_command_output_event(
    event: CommandOutputEvent,
    stdout: &mut String,
    stderr: &mut String,
    stdout_done: &mut bool,
    stderr_done: &mut bool,
    stream_sink: &mut Option<&mut ToolStreamSink<'_>>,
) {
    match event {
        CommandOutputEvent::Chunk(CommandOutputStream::Stdout, chunk) => {
            stdout.push_str(&chunk);
            emit_command_output_chunk(stream_sink, CommandOutputStream::Stdout, &chunk);
        }
        CommandOutputEvent::Chunk(CommandOutputStream::Stderr, chunk) => {
            stderr.push_str(&chunk);
            emit_command_output_chunk(stream_sink, CommandOutputStream::Stderr, &chunk);
        }
        CommandOutputEvent::ReadError(CommandOutputStream::Stdout, error) => {
            *stdout_done = true;
            stderr.push_str(&format!("\n[stdout read error] {}\n", error));
        }
        CommandOutputEvent::ReadError(CommandOutputStream::Stderr, error) => {
            *stderr_done = true;
            stderr.push_str(&format!("\n[stderr read error] {}\n", error));
        }
        CommandOutputEvent::Done(CommandOutputStream::Stdout) => {
            *stdout_done = true;
        }
        CommandOutputEvent::Done(CommandOutputStream::Stderr) => {
            *stderr_done = true;
        }
    }
}

fn drain_command_output_events(
    receiver: &mpsc::Receiver<CommandOutputEvent>,
    stdout: &mut String,
    stderr: &mut String,
    stdout_done: &mut bool,
    stderr_done: &mut bool,
    stream_sink: &mut Option<&mut ToolStreamSink<'_>>,
) {
    while let Ok(event) = receiver.try_recv() {
        handle_command_output_event(event, stdout, stderr, stdout_done, stderr_done, stream_sink);
    }
}

pub(crate) fn run_workspace_command_tool(
    workspace: &Path,
    arguments: &Value,
    mut stream_sink: Option<&mut ToolStreamSink<'_>>,
) -> Result<String, String> {
    let command = tool_arg_string(arguments, "command");
    let args = tool_arg_string_array(arguments, "args");
    let env_overrides = tool_arg_string_map(arguments, "env");
    let timeout_ms = tool_arg_usize(arguments, "timeoutMs", 30_000, 1_000, 120_000) as u64;
    let cwd_arg = tool_arg_string(arguments, "cwd");
    let cwd = if cwd_arg.is_empty() {
        workspace.to_path_buf()
    } else {
        resolve_workspace_relative_path(workspace, cwd_arg)?
    };

    if command.is_empty() {
        return Err("run_command requires a command.".to_string());
    }

    if command.contains('/') || command.contains('\\') {
        return Err("run_command command must be an executable name, not a path.".to_string());
    }

    if !cwd.is_dir() {
        return Err("run_command cwd must be a directory.".to_string());
    }

    let metadata = format!(
        "workspace={}\ncwd={}\nenv_overrides={}\n",
        workspace.display(),
        cwd.display(),
        env_overrides.len()
    );
    let mut command_builder = Command::new(command);
    command_builder
        .current_dir(&cwd)
        .args(&args)
        .envs(env_overrides.iter().map(|(name, value)| (name, value)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command_builder
        .spawn()
        .map_err(|error| format!("Failed to start command: {}", error))?;

    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture command stdout.".to_string())?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture command stderr.".to_string())?;
    let (sender, receiver) = mpsc::channel();
    let stdout_reader =
        spawn_command_output_reader(stdout_pipe, CommandOutputStream::Stdout, sender.clone());
    let stderr_reader =
        spawn_command_output_reader(stderr_pipe, CommandOutputStream::Stderr, sender);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut status = None;

    loop {
        drain_command_output_events(
            &receiver,
            &mut stdout,
            &mut stderr,
            &mut stdout_done,
            &mut stderr_done,
            &mut stream_sink,
        );

        if status.is_none() {
            status = child
                .try_wait()
                .map_err(|error| format!("Failed to poll command: {}", error))?;
        }

        if status.is_some() && stdout_done && stderr_done {
            break;
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child
                .wait()
                .map_err(|error| format!("Failed to wait for timed-out command: {}", error))?;

            while !(stdout_done && stderr_done) {
                match receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => handle_command_output_event(
                        event,
                        &mut stdout,
                        &mut stderr,
                        &mut stdout_done,
                        &mut stderr_done,
                        &mut stream_sink,
                    ),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            let combined = format!(
                "{}Command timed out after {} ms.\nstdout:\n{}\nstderr:\n{}",
                metadata,
                timeout_ms,
                stdout.trim(),
                stderr.trim()
            );
            return Err(combined);
        }

        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => handle_command_output_event(
                event,
                &mut stdout,
                &mut stderr,
                &mut stdout_done,
                &mut stderr_done,
                &mut stream_sink,
            ),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if status.is_some() {
                    break;
                }
            }
        }
    }

    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    drain_command_output_events(
        &receiver,
        &mut stdout,
        &mut stderr,
        &mut stdout_done,
        &mut stderr_done,
        &mut stream_sink,
    );

    let status = status.ok_or_else(|| "Command ended without an exit status.".to_string())?;
    let code = status
        .code()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "terminated".to_string());
    let combined = format!(
        "{}exit_code={}\nstdout:\n{}\nstderr:\n{}",
        metadata,
        code,
        stdout.trim(),
        stderr.trim()
    );

    if !status.success() {
        return Err(combined);
    }

    Ok(combined)
}
