pub_if_test!(mod drain);
pub_if_test!(mod evict);
mod utils;

use eyre::Result;
use kube::runtime::events::Recorder;

use crate::controllers::drain::start_drain_controller;
use crate::controllers::evict::start_evict_controller;
use crate::{
    ApiResolver, Config, LoadBalancingConfig, ServiceRegistry, Shutdown, Stores, pub_if_test,
};

/// Start a controller that deletes deregistered pods.
pub fn start_controllers(
    api_resolver: &ApiResolver,
    service_registry: &ServiceRegistry,
    loadbalancing: &LoadBalancingConfig,
    config: &Config,
    stores: &Stores,
    recorder: &Recorder,
    shutdown: &Shutdown,
) -> Result<()> {
    start_drain_controller(
        api_resolver,
        service_registry,
        loadbalancing,
        config,
        shutdown,
    )?;

    start_evict_controller(
        api_resolver,
        service_registry,
        loadbalancing,
        stores,
        recorder,
        shutdown,
    )?;

    Ok(())
}
