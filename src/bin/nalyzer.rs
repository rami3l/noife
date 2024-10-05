use std::{thread, time::Duration};

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("hello");
    let period = Duration::from_secs(3);
    loop {
        info!("running");
        thread::sleep(period)
    }
}
