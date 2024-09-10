use std::{
    env,
    ffi::CString,
    fs::File,
    os::{
        fd::{FromRawFd, RawFd},
        unix::ffi::OsStrExt,
    },
    process::{self, ExitCode},
};

use anyhow::{Context, Result};
use playground_common::{
    lock_exclusive, lock_shared, resolve_and_run_cmd, test_lock, unlock, RUSTUP_LOCK_FD,
};
use tracing::info;

fn main() -> Result<ExitCode> {
    tracing_subscriber::fmt::init();

    let (fd, is_root) = if let Ok(fd) = env::var(RUSTUP_LOCK_FD) {
        (RawFd::from_str_radix(&fd, 10)?, false)
    } else {
        let exe_path = env::current_exe()?;
        let exe_dir = exe_path
            .parent()
            .context("failed to get parent of current_exe")?;
        let lock_path = exe_dir.join(".rustup-lock-test");
        let lock_path = CString::new(lock_path.as_os_str().as_bytes())?;
        let fd = unsafe { libc::open(lock_path.as_ptr(), libc::O_RDWR | libc::O_CREAT) };
        (fd, true)
    };
    let lock = unsafe { File::from_raw_fd(fd) };
    let prefix = format!(
        "{} [{}]",
        if is_root { "root" } else { "child" },
        process::id()
    );

    let mut args = env::args_os();
    let _arg0 = args.next().context("no arg0 found")?;
    let args = args.collect::<Vec<_>>();

    // Rustup mode
    if matches!(&args[..], [head, ..] if head == "toolchain") {
        // if !is_root {
        //     info!("{prefix}: releasing lock");
        //     unlock(&lock)?;
        // }

        info!("{prefix}: acquiring write lock");
        test_lock(&lock)?;
        lock_exclusive(&lock)?;
        env::set_var(RUSTUP_LOCK_FD, fd.to_string());

        info!("{prefix}: CRITICAL SECTION");

        if is_root {
            info!("{prefix}: releasing lock");
            unlock(&lock)?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Proxy mode
    if is_root {
        info!("{prefix}: acquiring read lock");
        lock_shared(&lock)?;
    }
    env::set_var(RUSTUP_LOCK_FD, fd.to_string());

    info!("{prefix}: CRITICAL SECTION");

    // std::thread::sleep(std::time::Duration::from_secs(60));
    let code = resolve_and_run_cmd(&args)?.code().unwrap_or(0);

    if is_root {
        info!("{prefix}: releasing lock");
        unlock(&lock)?;
    }

    Ok(ExitCode::from(code as u8))
}
