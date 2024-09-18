use std::{thread, time::Duration};

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("hello");
    let period = Duration::from_secs(3);
    // TODO: Add proper handling for Ctrl-C so that the lock can be released properly.
    for _ in 0..3 {
        info!("running");
        thread::sleep(period)
    }
    Ok(())
}
