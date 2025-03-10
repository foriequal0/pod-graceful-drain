use std::time::Duration;

use clap::Parser;
use humantime::parse_duration;

#[derive(Clone, Debug, Parser)]
#[command(version, about)]
pub struct Config {
    #[arg(long, default_value = "60s", value_parser = parse_duration)]
    pub delete_after: Duration,

    #[arg(long, default_value = "false")]
    pub experimental_general_ingress: bool,
}
