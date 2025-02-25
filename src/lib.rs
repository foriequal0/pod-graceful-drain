mod api_resolver;
mod config;
mod consts;
mod controller;
mod downward_api;
mod elbv2;
mod impersonate;
mod loadbalancing;
mod pod_draining_info;
mod pod_evict_params;
mod pod_state;
mod reflector;
mod service_registry;
mod shutdown;
mod spawn_service;
mod status;
mod utils;
pub mod webhooks;

pub use crate::api_resolver::ApiResolver;
pub use crate::config::Config;
pub use crate::controller::start_controller;
pub use crate::downward_api::DownwardAPI;
pub use crate::loadbalancing::LoadBalancingConfig;
pub use crate::reflector::{start_reflectors, Stores};
pub use crate::service_registry::ServiceRegistry;
pub use crate::shutdown::Shutdown;
pub use crate::webhooks::{start_webhook, WebhookConfig};

#[cfg(test)]
pub use crate::webhooks::patch_pod_isolate;

#[cfg(test)]
#[macro_use]
extern crate assert_matches;
