use std::{
    env,
    ffi::OsStr,
    process::{self, ExitCode},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use playground_common::{localhost, resolve_and_run_cmd, resolve_cmd, RUSTUP_LOCK_ID};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    select, signal, time,
};
use tracing::info;

// TODO: Use Unix domain sockets instead of TCP sockets for this.
const LOCKER_PORT: u16 = 8080;

struct LockerClient {
    lock_id: u32,
    port: u16,
}

impl LockerClient {
    async fn read_lock(&self) -> Result<()> {
        let mut stream = TcpStream::connect(&localhost(self.port)[..]).await?;
        stream.write_u8(b'r').await?;
        stream.write_u32(self.lock_id).await?;
        stream.read_u8().await?;
        Ok(())
    }

    async fn write_lock(&self) -> Result<()> {
        let mut stream = TcpStream::connect(&localhost(self.port)[..]).await?;
        stream.write_u8(b'w').await?;
        stream.write_u32(self.lock_id).await?;
        stream.read_u8().await?;
        Ok(())
    }

    async fn unlock(&self) -> Result<()> {
        let mut stream = TcpStream::connect(&localhost(self.port)[..]).await?;
        stream.write_u8(b'u').await?;
        stream.write_u32(self.lock_id).await?;
        stream.read_u8().await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    tracing_subscriber::fmt::init();

    // Inherit the [`LockId`] from the root if it exists, or create it otherwise.
    let (lock_id, is_root) = if let Ok(lock_id) = env::var(RUSTUP_LOCK_ID) {
        (lock_id.parse()?, false)
    } else {
        resolve_cmd(OsStr::new("locker"))?
            .args([LOCKER_PORT.to_string()])
            .spawn()?;
        time::sleep(Duration::from_secs(1)).await;
        (process::id(), true)
    };

    env::set_var(RUSTUP_LOCK_ID, lock_id.to_string());

    let locker = Arc::new(LockerClient {
        lock_id,
        port: LOCKER_PORT,
    });

    select! {
        _ = signal::ctrl_c() => {
            info!("received SIGINT, shutting down...");

            let prefix = format!(
                "{} [{}]",
                if is_root { "root" } else { "child" },
                locker.lock_id
            );

            // Release the read lock if it's not inherited.
            if is_root {
                info!("{prefix}: releasing lock");
                _ = locker.unlock().await;
            }

            Ok(ExitCode::FAILURE)
        }
        res = run_nustup(Arc::clone(&locker), is_root) => res,
    }
}

async fn run_nustup(locker: Arc<LockerClient>, is_root: bool) -> Result<ExitCode> {
    let mut args = env::args_os();
    let _arg0 = args.next().context("no arg0 found")?;
    let args = args.collect::<Vec<_>>();

    let prefix = format!(
        "{} [{}]",
        if is_root { "root" } else { "child" },
        locker.lock_id
    );

    match &args[..] {
        // Rustup mode
        //
        // NOTE: For the sake of this demo, we consider `nustup toolchain`
        // the only occasion where a write lock is required.
        // This is to simulate the behavior of e.g. `rustup toolchain install`,
        // where the current Rust installation needs to be modified.
        [head, ..] if head == "toolchain" => {
            // Acquire (or upgrade to) the write lock.
            info!("{prefix}: acquiring write lock");
            locker.write_lock().await?;

            info!("{prefix}: CRITICAL SECTION");

            // Release (or downgrade from) the write lock.
            if is_root {
                info!("{prefix}: releasing lock");
                _ = locker.unlock().await;
            } else {
                info!("{prefix}: acquiring read lock");
                locker.read_lock().await?;
            }
            Ok(ExitCode::SUCCESS)
        }

        // Proxy mode
        //
        // NOTE: For the sake of this demo, we consider that other `nustup foo`
        // cases should launch a subprocess named `foo`.
        // This is to simulate the behavior of e.g. `cargo` and `rust-analyzer`
        // being launched via a corresponding rustup proxy.
        args @ [head, tail @ ..] => {
            // Acquire (or inherit) the read lock.
            if is_root {
                info!("{prefix}: acquiring read lock");
                locker.read_lock().await?;
            }

            info!("{prefix}: CRITICAL SECTION");

            // TODO: To do this more neatly, maybe make this a separate function.
            let code = if is_root {
                // NOTE: We take special care for the root rustup and forbid its use of
                // `exec*()` here even on Unix, since it is responsible for releasing
                // the lock when the following subprocess has exited.
                resolve_cmd(head)?.args(tail).status()?
            } else {
                resolve_and_run_cmd(args)?
            }
            .code()
            .unwrap_or(0);

            // Release the read lock if it's not inherited.
            if is_root {
                info!("{prefix}: releasing lock");
                _ = locker.unlock().await;
            }
            Ok(ExitCode::from(code as u8))
        }

        [] => bail!("no subcommand provided"),
    }
}
