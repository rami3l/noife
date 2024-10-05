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
    is_root: bool,
}

impl LockerClient {
    async fn read_raw(&self) -> Result<()> {
        let mut stream = TcpStream::connect(&localhost(self.port)[..]).await?;
        stream.write_u8(b'r').await?;
        stream.write_u32(self.lock_id).await?;
        stream.read_u8().await?;
        Ok(())
    }

    async fn write_raw(&self) -> Result<()> {
        let mut stream = TcpStream::connect(&localhost(self.port)[..]).await?;
        stream.write_u8(b'w').await?;
        stream.write_u32(self.lock_id).await?;
        stream.read_u8().await?;
        Ok(())
    }

    async fn unlock_raw(&self) -> Result<()> {
        let mut stream = TcpStream::connect(&localhost(self.port)[..]).await?;
        stream.write_u8(b'u').await?;
        stream.write_u32(self.lock_id).await?;
        stream.read_u8().await?;
        Ok(())
    }

    fn _prefix(&self) -> String {
        format!(
            "{} [{}]",
            if self.is_root { "root" } else { "child" },
            self.lock_id
        )
    }

    // Acquires (or inherits) a read lock.
    async fn read(&self) -> Result<()> {
        if self.is_root {
            info!("{}: acquiring read lock", self._prefix());
            self.read_raw().await?;
        }
        Ok(())
    }

    // Acquires (or upgrades to) the write lock.
    async fn write(&self) -> Result<()> {
        info!("{}: acquiring write lock", self._prefix());
        self.write_raw().await
    }

    /// Releases (or downgrades from) the write lock.
    async fn unwrite(&self) -> Result<()> {
        let prefix = self._prefix();
        if self.is_root {
            info!("{prefix}: releasing lock");
            _ = self.unlock_raw().await;
        } else {
            info!("{prefix}: acquiring read lock");
            self.read_raw().await?;
        }
        Ok(())
    }

    /// Releases the read lock if it's not inherited.
    async fn unread(&self) {
        if self.is_root {
            info!("{}: releasing lock", self._prefix());
            _ = self.unlock_raw().await;
        }
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
        is_root,
    });

    select! {
        _ = signal::ctrl_c() => {
            info!("received SIGINT, shutting down...");
            Ok(ExitCode::FAILURE)
        }
        res = run_nustup(Arc::clone(&locker)) => res,
    }
}

async fn run_nustup(locker: Arc<LockerClient>) -> Result<ExitCode> {
    let mut args = env::args_os();
    let _arg0 = args.next().context("no arg0 found")?;
    let args = args.collect::<Vec<_>>();

    let prefix = locker._prefix();

    match &args[..] {
        // Rustup mode
        //
        // NOTE: For the sake of this demo, we consider `nustup toolchain`
        // the only occasion where a write lock is required.
        // This is to simulate the behavior of e.g. `rustup toolchain install`,
        // where the current Rust installation needs to be modified.
        [head, ..] if head == "toolchain" => {
            locker.write().await?;

            info!("{prefix}: CRITICAL SECTION");

            locker.unwrite().await?;
            Ok(ExitCode::SUCCESS)
        }

        // Proxy mode
        //
        // NOTE: For the sake of this demo, we consider that other `nustup foo`
        // cases should launch a subprocess named `foo`.
        // This is to simulate the behavior of e.g. `cargo` and `rust-analyzer`
        // being launched via a corresponding rustup proxy.
        args @ [head, tail @ ..] => {
            locker.read().await?;

            info!("{prefix}: CRITICAL SECTION");

            // TODO: To do this more neatly, maybe make this a separate function.
            let code = if locker.is_root {
                // NOTE: We take special care for the root rustup and forbid its use of
                // `exec*()` here even on Unix, since it is responsible for releasing
                // the lock when the following subprocess has exited.
                resolve_cmd(head)?.args(tail).status()?
            } else {
                resolve_and_run_cmd(args)?
            }
            .code()
            .unwrap_or(0);

            locker.unread().await;
            Ok(ExitCode::from(code as u8))
        }

        [] => bail!("no subcommand provided"),
    }
}
