use std::time::Duration;

use clap::Parser;
use eyre::{Result, eyre};
use humantime::parse_duration;

#[derive(Clone, Debug, Parser)]
#[command(version, about)]
pub struct Config {
    #[arg(long, default_value = "25s", value_parser = parse_delete_after)]
    pub delete_after: Duration,

    #[arg(long, default_value = "false")]
    pub experimental_general_ingress: bool,
}

fn parse_delete_after(input: &str) -> Result<Duration> {
    let duration = parse_duration(input)?;
    if duration > Duration::from_secs(25) {
        return Err(eyre!("delete-after should be >=1s, <= 25s"));
    }

    Ok(duration)
}
