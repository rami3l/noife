use std::{env, ffi::OsStr};

use anyhow::{Context, Result};
use playground_common::{resolve_and_run_cmd, resolve_cmd};
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("hello");
    let mut args = env::args_os();
    let _arg0 = args.next().context("no arg0 found")?;
    let args = args.collect::<Vec<_>>();
    match &args[..] {
        [head, ..] if head == "rec" => {
            resolve_and_run_cmd(&["nustup", "nargo"])?;
        }
        [head, ..] if head == "spawn" => {
            resolve_cmd(OsStr::new("nustup"))?
                .args(["toolchain"])
                .status()?;
        }
        _ => (),
    };
    Ok(())
}
