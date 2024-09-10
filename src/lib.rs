use std::{
    env,
    ffi::OsStr,
    fmt::Debug,
    io,
    process::{self, Command, ExitStatus},
};

use anyhow::{anyhow, bail, Context, Result};
use tracing::info;

pub fn resolve_and_run_cmd(args: &[impl AsRef<OsStr> + Debug]) -> Result<ExitStatus> {
    let [head, tail @ ..] = args else {
        bail!("no arg0 found");
    };
    let cmd = resolve_cmd(head.as_ref())?;
    run_command_for_dir(cmd, head, tail)
}

pub fn resolve_cmd(arg0: &OsStr) -> Result<Command> {
    let exe_path = env::current_exe()?;
    let dir = exe_path
        .parent()
        .context("failed to get parent of current_exe")?;
    info!("creating command `{arg0:?}` under `{dir:?}`");
    Ok(Command::new(dir.join(arg0)))
}

// https://github.com/rust-lang/rustup/blob/15a7ec40f00459140d444cd235af4e090a2602da/src/command.rs#L13-L56
pub fn run_command_for_dir<S: AsRef<OsStr> + Debug>(
    mut cmd: Command,
    arg0: &S,
    args: &[S],
) -> Result<ExitStatus> {
    cmd.args(args);

    // FIXME rust-lang/rust#32254. It's not clear to me
    // when and why this is needed.
    // TODO: process support for mocked file descriptor inheritance here: until
    // then tests that depend on rustups stdin being inherited won't work in-process.
    cmd.stdin(process::Stdio::inherit());

    return exec(&mut cmd)
        .with_context(|| anyhow!("error running command `{arg0:?}` with `{args:?}`"));

    #[cfg(unix)]
    fn exec(cmd: &mut Command) -> io::Result<ExitStatus> {
        use std::os::unix::prelude::*;
        Err(cmd.exec())
    }

    #[cfg(windows)]
    fn exec(cmd: &mut Command) -> io::Result<ExitStatus> {
        use windows_sys::Win32::Foundation::{BOOL, FALSE, TRUE};
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

        unsafe extern "system" fn ctrlc_handler(_: u32) -> BOOL {
            // Do nothing. Let the child process handle it.
            TRUE
        }
        unsafe {
            if SetConsoleCtrlHandler(Some(ctrlc_handler), TRUE) == FALSE {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Unable to set console handler",
                ));
            }
        }

        cmd.status()
    }
}
