use std::collections::HashMap;
use std::default::Default;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard};

use async_shutdown::ShutdownManager;
use strum::IntoEnumIterator;

#[derive(Clone)]
pub struct StagedShutdown<T> {
    inner: Arc<Mutex<HashMap<T, ShutdownManager<()>>>>,
}

pub type ShutdownSignal = async_shutdown::ShutdownSignal<()>;
pub type ShutdownComplete = async_shutdown::ShutdownComplete<()>;
pub type ShutdownAlreadyCompleted = async_shutdown::ShutdownAlreadyCompleted<()>;
pub type WrapDelayShutdown<F> = async_shutdown::WrapDelayShutdown<(), F>;

impl<T> StagedShutdown<T>
where
    T: Eq + Hash + IntoEnumIterator,
{
    pub fn new() -> StagedShutdown<T> {
        let mut map = HashMap::new();
        for e in T::iter() {
            map.insert(e, ShutdownManager::new());
        }

        StagedShutdown {
            inner: Arc::new(Mutex::new(map)),
        }
    }
}

impl<T> Default for StagedShutdown<T>
where
    T: Eq + Hash + IntoEnumIterator,
{
    fn default() -> Self {
        StagedShutdown::new()
    }
}

impl<T> StagedShutdown<T>
where
    T: Eq + Hash,
{
    pub fn is_triggered(&self, stage: T) -> bool {
        let inner = self.inner.lock().expect("Lock poisoned");
        inner[&stage].is_shutdown_triggered()
    }

    pub fn wait_triggered(&self, stage: T) -> ShutdownSignal {
        let inner = self.get_inner();
        inner[&stage].wait_shutdown_triggered()
    }

    pub fn wait_complete(&self, stage: T) -> ShutdownComplete {
        let inner = self.get_inner();
        inner[&stage].wait_shutdown_complete()
    }

    pub fn trigger(&self, stage: T) {
        let inner = self.get_inner();
        _ = inner[&stage].trigger_shutdown(());
    }

    pub fn wrap_delay<F: Future>(
        &self,
        stage: T,
        future: F,
    ) -> Result<WrapDelayShutdown<F>, ShutdownAlreadyCompleted> {
        let inner = self.get_inner();
        Ok(inner[&stage].delay_shutdown_token()?.wrap_future(future))
    }

    fn get_inner(&self) -> MutexGuard<HashMap<T, ShutdownManager<()>>> {
        self.inner.lock().expect("Lock poisoned")
    }
}
