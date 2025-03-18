mod api_resolver;
mod configs;
mod controllers;
mod downward_api;
mod elbv2;
mod error_codes;
mod labels_and_annotations;
mod loadbalancing;
mod patch;
mod pod_disruption_budget;
mod pod_state;
mod reflector;
mod selector;
mod service_registry;
mod shutdown;
mod spawn_service;
mod utils;
pub mod webhooks;

mod error_types;
mod report;
#[cfg(test)]
mod tests;

pub const CONTROLLER_NAME: &str = "pod-graceful-drain";

pub use crate::api_resolver::ApiResolver;
pub use crate::configs::Config;
pub use crate::controllers::start_controllers;
pub use crate::downward_api::DownwardAPI;
pub use crate::loadbalancing::LoadBalancingConfig;
pub use crate::reflector::{Stores, start_reflectors};
pub use crate::service_registry::ServiceRegistry;
pub use crate::shutdown::Shutdown;
pub use crate::webhooks::{WebhookConfig, start_webhook};

#[cfg(test)]
#[macro_use]
extern crate assert_matches;
