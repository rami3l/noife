use std::{
    env,
    ffi::OsStr,
    fmt::Debug,
    fs::File,
    io,
    mem::MaybeUninit,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    os::fd::AsRawFd,
    process::{self, Command, ExitStatus},
};

use anyhow::{anyhow, bail, Context, Result};
use tracing::{error, info};

pub const RUSTUP_LOCK_ID: &str = "RUSTUP_LOCK_ID";

pub fn localhost(port: u16) -> [SocketAddr; 2] {
    [
        SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, port, 0, 0)),
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
    ]
}

pub fn lock_shared(file: &File) -> io::Result<()> {
    flock(file, libc::LOCK_SH)
}

pub fn lock_exclusive(file: &File) -> io::Result<()> {
    flock(file, libc::LOCK_EX)
}

pub fn try_lock_shared(file: &File) -> io::Result<()> {
    flock(file, libc::LOCK_SH | libc::LOCK_NB)
}

pub fn try_lock_exclusive(file: &File) -> io::Result<()> {
    flock(file, libc::LOCK_EX | libc::LOCK_NB)
}

pub fn unlock(file: &File) -> io::Result<()> {
    flock(file, libc::LOCK_UN)
}

// fn flock(file: &File, flag: libc::c_int) -> io::Result<()> {
//     let ret = unsafe { libc::flock(file.as_raw_fd(), flag) };
//     if ret < 0 {
//         Err(io::Error::last_os_error())
//     } else {
//         Ok(())
//     }
// }

fn flock(file: &File, flag: libc::c_int) -> io::Result<()> {
    // Solaris lacks flock(), so try to emulate using fcntl()
    let mut flock: libc::flock = unsafe { MaybeUninit::zeroed().assume_init() };
    flock.l_type = if flag & libc::LOCK_UN != 0 {
        libc::F_UNLCK
    } else if flag & libc::LOCK_EX != 0 {
        libc::F_WRLCK
    } else if flag & libc::LOCK_SH != 0 {
        libc::F_RDLCK
    } else {
        panic!("unexpected flock() operation")
    };

    let mut cmd = libc::F_SETLKW;
    if (flag & libc::LOCK_NB) != 0 {
        cmd = libc::F_SETLK;
    }

    let ret = unsafe { libc::fcntl(file.as_raw_fd(), cmd, &flock) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn test_lock(file: &File) -> io::Result<()> {
    // Solaris lacks flock(), so try to emulate using fcntl()
    let mut flock: libc::flock = unsafe { MaybeUninit::zeroed().assume_init() };
    flock.l_type = libc::F_WRLCK;

    let ret = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &flock) };
    match flock.l_type {
        libc::F_RDLCK => error!("file has been read-locked by {}", flock.l_pid),
        libc::F_WRLCK => error!("file has been write-locked by {}", flock.l_pid),
        _ => (),
    }
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

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
    info!("creating command {arg0:?} under {dir:?}");
    Ok(Command::new(dir.join(arg0)))
}

// https://github.com/rust-lang/rustup/blob/15a7ec40f00459140d444cd235af4e090a2602da/src/command.rs#L13-L56
pub fn run_command_for_dir<S: AsRef<OsStr> + Debug>(
    mut cmd: Command,
    arg0: &S,
    args: &[S],
) -> Result<ExitStatus> {
    #[cfg(unix)]
    fn exec(cmd: &mut Command) -> io::Result<ExitStatus> {
        // use std::os::unix::prelude::*;
        // Err(cmd.exec())

        // NOTE: Looks like we can no longer use `exec` anyway,
        // otherwise we might not be able to clean up.
        cmd.status()
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

    cmd.args(args);

    // FIXME rust-lang/rust#32254. It's not clear to me
    // when and why this is needed.
    // TODO: process support for mocked file descriptor inheritance here: until
    // then tests that depend on rustups stdin being inherited won't work in-process.
    cmd.stdin(process::Stdio::inherit());

    exec(&mut cmd).with_context(|| anyhow!("error running command `{arg0:?}` with `{args:?}`"))
}
