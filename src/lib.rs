mod api_resolver;
mod config;
mod consts;
mod controller;
mod downward_api;
mod elbv2;
mod error_codes;
mod loadbalancing;
mod patch;
mod pod_draining_info;
mod pod_evict_params;
mod pod_state;
mod reflector;
mod service_registry;
mod shutdown;
mod spawn_service;
mod utils;
pub mod webhooks;

pub use crate::api_resolver::ApiResolver;
pub use crate::config::Config;
pub use crate::controller::start_controller;
pub use crate::downward_api::DownwardAPI;
pub use crate::loadbalancing::LoadBalancingConfig;
pub use crate::reflector::{Stores, start_reflectors};
pub use crate::service_registry::ServiceRegistry;
pub use crate::shutdown::Shutdown;
pub use crate::webhooks::{WebhookConfig, start_webhook};

// public for test
pub use crate::patch::patch_pod_isolate;

#[cfg(test)]
#[macro_use]
extern crate assert_matches;
