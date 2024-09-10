use std::env;

use anyhow::{Context, Result};
use playground_common::resolve_and_run_cmd;
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("hello");
    let mut args = env::args_os();
    let _arg0 = args.next().context("no arg0 found")?;
    let args = args.collect::<Vec<_>>();
    if matches!(&args[..], [head, ..] if head == "rec") {
        resolve_and_run_cmd(&["nustup", "nargo"])?;
    }
    Ok(())
}
