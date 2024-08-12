use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::debug;

#[derive(Clone)]
pub struct ServiceRegistry {
    services: Arc<Mutex<Vec<Arc<ServiceState>>>>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self {
            services: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ServiceRegistry {
    pub fn register(&self, name: &str) -> ServiceSignal {
        let state = Arc::new(ServiceState {
            name: name.to_string(),
            ready: AtomicBool::new(false),
        });

        let mut services = self.services.lock().unwrap();
        services.push(Arc::clone(&state));
        debug!(%name, "Service registered");
        ServiceSignal { state }
    }

    pub fn get_not_ready_services(&self) -> Vec<String> {
        let services = self.services.lock().unwrap();
        let mut result = Vec::new();
        for service in services.iter() {
            if !service.ready.load(Ordering::SeqCst) {
                result.push(service.name.clone());
            }
        }

        result
    }
}

pub struct ServiceSignal {
    state: Arc<ServiceState>,
}

impl ServiceSignal {
    pub fn ready(&self) {
        self.state.ready.store(true, Ordering::SeqCst);
        debug!(%self.state.name, "Service ready");
    }
}

struct ServiceState {
    name: String,
    ready: AtomicBool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_be_ready_after_all_registered_service_ready() {
        let registry = ServiceRegistry::default();
        let signal = registry.register("test");

        assert!(!registry.get_not_ready_services().is_empty());
        signal.ready();
        assert!(registry.get_not_ready_services().is_empty());
    }
}
