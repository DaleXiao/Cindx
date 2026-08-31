use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use agent_core::{sandboxed_argv, SandboxMode};

use crate::process_capture::{ProcessCapture, ProcessStopCause};
use crate::process_control::terminate_process_group;
use crate::process_cpu::process_group_cpu_nanos;
use crate::process_runtime::ProcessBudgets;
use crate::shell::configure_shell_environment;
use crate::ToolExecutionControl;

const INPUT_QUEUE_DEPTH: usize = 4;
const MONITOR_INTERVAL: Duration = Duration::from_millis(40);
const STOP_GRACE: Duration = Duration::from_millis(120);

#[cfg(unix)]
const PROCESS_WRAPPER: &str = r#"
parent_pid=$1
user_command=$2
group_id=$$
unsetopt bg_nice
(
  while kill -0 "$parent_pid" 2>/dev/null; do sleep 1; done
  kill -TERM -"$group_id" 2>/dev/null
  sleep 1
  kill -KILL -"$group_id" 2>/dev/null
) &
exec /bin/zsh -fc "$user_command"
"#;

pub(crate) struct ProcessInputRequest {
    pub(crate) bytes: Vec<u8>,
    pub(crate) close: bool,
    pub(crate) acknowledgement: mpsc::Sender<Result<usize, String>>,
}

pub(crate) struct SupervisedProcess {
    pub(crate) stop_sender: mpsc::Sender<ProcessStopCause>,
    pub(crate) monitor: JoinHandle<()>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_supervised_process(
    command: &str,
    cwd: &Path,
    budgets: ProcessBudgets,
    control: ToolExecutionControl,
    capture: Arc<ProcessCapture>,
    input_slot: Arc<Mutex<Option<mpsc::SyncSender<ProcessInputRequest>>>>,
    sandbox: SandboxMode,
    workspace_root: &Path,
) -> Result<SupervisedProcess, String> {
    let mut child = spawn_process(command, cwd, budgets.cpu_seconds, sandbox, workspace_root)?;
    let process_id = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    let (stop_sender, stop_receiver) = mpsc::channel();
    let (input_sender, input_receiver) = mpsc::sync_channel(INPUT_QUEUE_DEPTH);
    if let Ok(mut slot) = input_slot.lock() {
        *slot = Some(input_sender);
    } else {
        let _ = stop_child(&mut child, process_id);
        return Err("process input state is unavailable".to_string());
    }
    capture.mark_running();
    let monitor = thread::spawn(move || {
        supervise_process(
            child,
            process_id,
            stdout,
            stderr,
            stdin,
            input_receiver,
            input_slot,
            stop_receiver,
            capture,
            budgets,
            control,
        )
    });
    Ok(SupervisedProcess {
        stop_sender,
        monitor,
    })
}

#[allow(clippy::too_many_arguments)]
fn supervise_process(
    mut child: Child,
    process_id: u32,
    stdout: Option<impl Read + Send + 'static>,
    stderr: Option<impl Read + Send + 'static>,
    stdin: Option<impl Write + Send + 'static>,
    input_receiver: mpsc::Receiver<ProcessInputRequest>,
    input_slot: Arc<Mutex<Option<mpsc::SyncSender<ProcessInputRequest>>>>,
    stop_receiver: mpsc::Receiver<ProcessStopCause>,
    capture: Arc<ProcessCapture>,
    budgets: ProcessBudgets,
    control: ToolExecutionControl,
) {
    let stdout_reader =
        stdout.map(|stream| spawn_stream_reader(stream, Arc::clone(&capture), true));
    let stderr_reader =
        stderr.map(|stream| spawn_stream_reader(stream, Arc::clone(&capture), false));
    let stdin_writer = stdin.map(|stream| spawn_stdin_writer(stream, input_receiver));
    let deadline = Instant::now() + Duration::from_secs(budgets.timeout_seconds);
    let (status, forced_cause) = loop {
        if let Ok(cause) = stop_receiver.try_recv() {
            break (stop_child(&mut child, process_id), Some(cause));
        }
        if control.should_cancel() {
            break (
                stop_child(&mut child, process_id),
                Some(ProcessStopCause::Cancelled),
            );
        }
        if output_limit_reached(&capture, budgets.output_limit_bytes) {
            break (
                stop_child(&mut child, process_id),
                Some(ProcessStopCause::OutputLimit),
            );
        }
        if process_group_cpu_nanos(process_id)
            .is_some_and(|used| used >= budgets.cpu_seconds.saturating_mul(1_000_000_000))
        {
            break (
                stop_child(&mut child, process_id),
                Some(ProcessStopCause::CpuLimit),
            );
        }
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), None),
            Ok(None) if Instant::now() < deadline => thread::sleep(MONITOR_INTERVAL),
            Ok(None) => {
                break (
                    stop_child(&mut child, process_id),
                    Some(ProcessStopCause::Timeout),
                )
            }
            Err(_) => {
                break (
                    stop_child(&mut child, process_id),
                    Some(ProcessStopCause::Signal),
                )
            }
        }
    };

    terminate_process_group(process_id, 15);
    thread::sleep(Duration::from_millis(20));
    terminate_process_group(process_id, 9);
    if let Ok(mut slot) = input_slot.lock() {
        slot.take();
    }
    if let Some(writer) = stdin_writer {
        let _ = writer.join();
    }
    if let Some(reader) = stdout_reader {
        let _ = reader.join();
    } else {
        capture.mark_stream_closed(true);
    }
    if let Some(reader) = stderr_reader {
        let _ = reader.join();
    } else {
        capture.mark_stream_closed(false);
    }

    let exit_code = status.as_ref().and_then(ExitStatus::code);
    let signal = status.as_ref().and_then(exit_signal);
    let cause = forced_cause.unwrap_or_else(|| {
        if signal == Some(cpu_limit_signal()) {
            ProcessStopCause::CpuLimit
        } else if exit_code.is_some() {
            ProcessStopCause::Exit
        } else {
            ProcessStopCause::Signal
        }
    });
    capture.finish(cause, exit_code, signal);
}

/// The argv used to launch a supervised process under the session's sandbox
/// mode. [`SandboxMode::FullAccess`] returns the exact historical argv; every
/// other mode wraps the wrapper shell in `/usr/bin/sandbox-exec`, so managed
/// processes honor the same confinement as `shell.run`.
#[cfg(unix)]
pub(crate) fn process_spawn_argv(
    command: &str,
    sandbox: SandboxMode,
    workspace_root: &Path,
) -> Vec<String> {
    let inner = vec![
        "/bin/zsh".to_string(),
        "-fc".to_string(),
        PROCESS_WRAPPER.to_string(),
        "cindx-process".to_string(),
        std::process::id().to_string(),
        command.to_string(),
    ];
    sandboxed_argv(sandbox, workspace_root, inner)
}

fn spawn_process(
    command: &str,
    cwd: &Path,
    cpu_seconds: u64,
    sandbox: SandboxMode,
    workspace_root: &Path,
) -> Result<Child, String> {
    #[cfg(unix)]
    let argv = process_spawn_argv(command, sandbox, workspace_root);
    #[cfg(not(unix))]
    let argv = {
        let _ = (sandbox, workspace_root);
        vec![
            "/bin/zsh".to_string(),
            "-fc".to_string(),
            command.to_string(),
        ]
    };
    let mut process = Command::new(&argv[0]);
    process.args(&argv[1..]);
    process
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_shell_environment(&mut process);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
        let soft = cpu_seconds as libc::rlim_t;
        let hard = cpu_seconds.saturating_add(1) as libc::rlim_t;
        unsafe {
            process.pre_exec(move || {
                let limit = libc::rlimit {
                    rlim_cur: soft,
                    rlim_max: hard,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    process
        .spawn()
        .map_err(|error| format!("failed to start process: {error}"))
}

fn spawn_stream_reader(
    mut stream: impl Read + Send + 'static,
    capture: Arc<ProcessCapture>,
    stdout: bool,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) if stdout => {
                    capture.append_stdout(&buffer[..count]);
                }
                Ok(count) => {
                    capture.append_stderr(&buffer[..count]);
                }
                Err(_) => break,
            }
        }
        capture.mark_stream_closed(stdout);
    })
}

fn spawn_stdin_writer(
    mut stdin: impl Write + Send + 'static,
    receiver: mpsc::Receiver<ProcessInputRequest>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(request) = receiver.recv() {
            let result = stdin
                .write_all(&request.bytes)
                .and_then(|_| stdin.flush())
                .map(|_| request.bytes.len())
                .map_err(|error| error.to_string());
            let _ = request.acknowledgement.send(result);
            if request.close {
                break;
            }
        }
    })
}

fn stop_child(child: &mut Child, process_id: u32) -> Option<ExitStatus> {
    terminate_process_group(process_id, 15);
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    terminate_process_group(process_id, 9);
    let _ = child.kill();
    child.wait().ok()
}

fn output_limit_reached(capture: &ProcessCapture, limit: u64) -> bool {
    capture
        .wait_and_snapshot(0, 0, 0, Duration::ZERO)
        .map(|snapshot| {
            snapshot
                .stdout
                .produced_bytes
                .saturating_add(snapshot.stderr.produced_bytes)
                > limit
        })
        .unwrap_or(true)
}

#[cfg(unix)]
fn exit_signal(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

#[cfg(unix)]
const fn cpu_limit_signal() -> i32 {
    libc::SIGXCPU
}

#[cfg(not(unix))]
const fn cpu_limit_signal() -> i32 {
    -1
}
