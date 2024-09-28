use std::{
    env,
    ffi::OsStr,
    process::{self, ExitCode},
};

use anyhow::{bail, Context, Result};
use playground_common::{localhost, resolve_and_run_cmd, resolve_cmd, RUSTUP_LOCK_ID};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tracing::info;

// TODO: Figure out a better port no. scheme
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

    let mut args = env::args_os();
    let _arg0 = args.next().context("no arg0 found")?;
    let args = args.collect::<Vec<_>>();

    let (lock_id, is_root) = if let Ok(lock_id) = env::var(RUSTUP_LOCK_ID) {
        (lock_id.parse()?, false)
    } else {
        resolve_cmd(OsStr::new("locker"))?
            .args([LOCKER_PORT.to_string()])
            .spawn()?;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        (process::id(), true)
    };
    env::set_var(RUSTUP_LOCK_ID, lock_id.to_string());

    let prefix = format!("{} [{lock_id}]", if is_root { "root" } else { "child" });

    let locker = LockerClient {
        lock_id,
        port: LOCKER_PORT,
    };

    match &args[..] {
        // Rustup mode
        [head, ..] if head == "toolchain" => {
            // if !is_root {
            //     info!("{prefix}: releasing lock");
            //     locker.unlock().await?;
            // }

            info!("{prefix}: acquiring write lock");
            locker.write_lock().await?;

            info!("{prefix}: CRITICAL SECTION");

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
        args @ [head, tail @ ..] => {
            if is_root {
                info!("{prefix}: acquiring read lock");
                locker.read_lock().await?;
            }

            info!("{prefix}: CRITICAL SECTION");

            // std::thread::sleep(std::time::Duration::from_secs(60));
            let code = if is_root {
                resolve_cmd(head)?.args(tail).status()?
            } else {
                resolve_and_run_cmd(args)?
            }
            .code()
            .unwrap_or(0);

            if is_root {
                info!("{prefix}: releasing lock");
                _ = locker.unlock().await;
            }
            Ok(ExitCode::from(code as u8))
        }

        [] => bail!("no subcommand provided"),
    }
}
