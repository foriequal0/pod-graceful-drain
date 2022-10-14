use std::net::SocketAddr;

use eyre::Result;
use rand::Rng;
use tokio::net::TcpListener;

use crate::webhooks::config::BindConfig;

pub async fn try_bind(bind_config: &BindConfig) -> Result<TcpListener> {
    match bind_config {
        BindConfig::SocketAddr(bind_addr) => {
            let incoming = TcpListener::bind(bind_addr).await?;
            Ok(incoming)
        }
        BindConfig::RandomForTest => {
            let mut retry = 0;
            loop {
                let random_port = rand::thread_rng().gen_range(49152..=65535);
                let bind_addr = SocketAddr::from(([0, 0, 0, 0], random_port));

                match TcpListener::bind(bind_addr).await {
                    Ok(incoming) => return Ok(incoming),
                    Err(err) => {
                        retry += 1;
                        if retry < 10 {
                            continue;
                        }

                        return Err(err.into());
                    }
                }
            }
        }
    }
}
