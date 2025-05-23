use std::default::Default;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;

use eyre::Result;
use futures::{Stream, StreamExt, TryStreamExt};
use k8s_openapi::Resource;
use k8s_openapi::api::core::v1::{PodSpec, PodStatus};
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use k8s_openapi::api::{
    core::v1::{Pod, Service},
    networking::v1::Ingress,
};
use kube::Api;
use kube::runtime::reflector::store::Writer;
use kube::runtime::reflector::{ObjectRef, Store, store};
use kube::runtime::watcher::Event;
use kube::runtime::{WatchStreamExt, watcher};
use tracing::{Level, debug, error, span, trace};

use crate::api_resolver::ApiResolver;
use crate::elbv2::apis::TargetGroupBinding;
use crate::error_codes::is_410_expired_error_response;
use crate::service_registry::ServiceSignal;
use crate::shutdown::Shutdown;
use crate::spawn_service::spawn_service;
use crate::{Config, ServiceRegistry, try_some};

#[derive(Clone)]
pub struct Stores {
    inner: Arc<StoresInner>,
}

pub struct StoresInner {
    pods: Store<Pod>,
    services: Store<Service>,
    ingresses: Store<Ingress>,
    pdbs: Store<PodDisruptionBudget>,
    tgbs: Store<TargetGroupBinding>,
}

impl Stores {
    pub(crate) fn new(
        pods: Store<Pod>,
        services: Store<Service>,
        ingresses: Store<Ingress>,
        pdbs: Store<PodDisruptionBudget>,
        tgbs: Store<TargetGroupBinding>,
    ) -> Self {
        Self {
            inner: Arc::new(StoresInner {
                pods,
                services,
                ingresses,
                pdbs,
                tgbs,
            }),
        }
    }
}

pub fn start_reflectors(
    api_resolver: &ApiResolver,
    config: &Config,
    service_registry: &ServiceRegistry,
    shutdown: &Shutdown,
) -> Result<Stores> {
    let (pod_reader, pod_writer) = store();
    spawn_service(
        shutdown,
        span!(Level::INFO, "reflector", kind = Pod::KIND),
        {
            let api: Api<Pod> = api_resolver.all();
            let stream = watcher(api, Default::default()).map_ok(|event| {
                event.modify(|pod| {
                    pod.metadata.managed_fields = None;
                    if let Some(spec) = try_some!(mut pod.spec?) {
                        *spec = PodSpec {
                            readiness_gates: spec.readiness_gates.clone(),
                            ..PodSpec::default()
                        }
                    }
                    if let Some(spec) = try_some!(mut pod.status?) {
                        *spec = PodStatus {
                            conditions: spec.conditions.clone(),
                            ..PodStatus::default()
                        }
                    }
                })
            });
            let signal = service_registry.register("reflector:Pod");
            run_reflector(shutdown, pod_writer, stream, signal)
        },
    )?;

    let (service_reader, service_writer) = store();
    spawn_service(
        shutdown,
        span!(Level::INFO, "reflector", kind = Service::KIND),
        {
            let api: Api<Service> = api_resolver.all();
            let stream = watcher(api, Default::default()).map_ok(|ev| {
                ev.modify(|service| {
                    service.metadata.annotations = None;
                    service.metadata.labels = None;
                    service.metadata.managed_fields = None;
                    service.status = None;
                })
            });
            let signal = service_registry.register("reflector:Service");
            run_reflector(shutdown, service_writer, stream, signal)
        },
    )?;

    let (ingress_reader, ingress_writer) = store();
    spawn_service(
        shutdown,
        span!(Level::INFO, "reflector", kind = Ingress::KIND),
        {
            let api: Api<Ingress> = api_resolver.all();
            let stream = watcher(api, Default::default()).map_ok(|ev| {
                ev.modify(|ingress| {
                    ingress.metadata.annotations = None;
                    ingress.metadata.labels = None;
                    ingress.metadata.managed_fields = None;
                    ingress.status = None;
                })
            });
            let signal = service_registry.register("reflector:Ingress");
            run_reflector(shutdown, ingress_writer, stream, signal)
        },
    )?;

    let (pdb_reader, pdb_writer) = store();
    spawn_service(
        shutdown,
        span!(Level::INFO, "reflector", kind = PodDisruptionBudget::KIND),
        {
            let api: Api<PodDisruptionBudget> = api_resolver.all();
            let stream = watcher(api, Default::default()).map_ok(|ev| {
                ev.modify(|ingress| {
                    ingress.metadata.annotations = None;
                    ingress.metadata.labels = None;
                    ingress.metadata.managed_fields = None;
                })
            });
            let signal = service_registry.register("reflector:PodDisruptionBudget");
            run_reflector(shutdown, pdb_writer, stream, signal)
        },
    )?;

    let (tgb_reader, tgb_writer) = store();
    if !config.experimental_general_ingress {
        spawn_service(
            shutdown,
            span!(Level::INFO, "reflector", kind = TargetGroupBinding::KIND),
            {
                let api: Api<TargetGroupBinding> = api_resolver.all();
                let stream = watcher(api, Default::default()).map_ok(|ev| {
                    ev.modify(|tgb| {
                        tgb.metadata.annotations = None;
                        tgb.metadata.labels = None;
                        tgb.metadata.managed_fields = None;
                        tgb.status = None;
                    })
                });
                let signal = service_registry.register("reflector:TargetGroupBinding");
                run_reflector(shutdown, tgb_writer, stream, signal)
            },
        )?;
    }

    Ok(Stores::new(
        pod_reader,
        service_reader,
        ingress_reader,
        pdb_reader,
        tgb_reader,
    ))
}

fn run_reflector<K>(
    shutdown: &Shutdown,
    writer: Writer<K>,
    stream: impl Stream<Item = watcher::Result<Event<K>>> + 'static,
    signal: ServiceSignal,
) -> impl Future<Output = ()> + 'static
where
    K: kube::Resource + Resource + Clone,
    K::DynamicType: Default + Eq + Hash + Clone,
{
    let shutdown = shutdown.clone();
    async move {
        let stream = stream
            .default_backoff()
            .take_until(shutdown.wait_shutdown_triggered());

        let mut results = Box::pin(kube::runtime::reflector(writer, stream));

        // Log until Event::InitDone
        while let Some(result) = results.next().await {
            log(&result, true);

            // TODO : raise appropriate signal when Event::Init restarted
            if let Ok(Event::InitDone) = result {
                signal.ready();
                break;
            }
        }

        while let Some(result) = results.next().await {
            log(&result, false);
        }

        fn log<K>(result: &watcher::Result<Event<K>>, init: bool)
        where
            K: kube::Resource,
            K::DynamicType: Default,
        {
            match result {
                Ok(event) => match event {
                    Event::Apply(resource) => {
                        let object_ref = ObjectRef::from_obj(resource);
                        trace!(%object_ref, "resource applied");
                    }
                    Event::Delete(resource) => {
                        let object_ref = ObjectRef::from_obj(resource);
                        trace!(%object_ref, "resource deleted");
                    }
                    Event::Init => {
                        trace!("stream restart");
                    }
                    Event::InitApply(resource) => {
                        let object_ref = ObjectRef::from_obj(resource);
                        trace!(%object_ref, "stream restarting");
                    }
                    Event::InitDone => {
                        trace!("stream restart done");
                    }
                },
                Err(watcher::Error::WatchFailed(err)) if !init => {
                    debug!(?err, "watch failed. stream will restart soon");
                }
                Err(watcher::Error::WatchError(resp))
                    if !init && is_410_expired_error_response(resp) =>
                {
                    debug!(?resp, "watch error. stream will restart");
                }
                Err(err) => {
                    error!(?err, "reflector error");
                }
            }
        }
    }
}

impl Stores {
    pub fn get_pod(&self, key: &ObjectRef<Pod>) -> Option<Arc<Pod>> {
        self.inner.pods.get(key)
    }

    pub fn get_service(&self, key: &ObjectRef<Service>) -> Option<Arc<Service>> {
        self.inner.services.get(key)
    }

    pub fn services(&self, ns: &str) -> Vec<Arc<Service>> {
        self.inner
            .services
            .state()
            .into_iter()
            .filter(|x| x.metadata.namespace.as_deref() == Some(ns))
            .collect()
    }

    pub fn ingresses(&self, ns: &str) -> Vec<Arc<Ingress>> {
        self.inner
            .ingresses
            .state()
            .into_iter()
            .filter(|x| x.metadata.namespace.as_deref() == Some(ns))
            .collect()
    }

    pub fn pod_disruption_budgets(&self, ns: &str) -> Vec<Arc<PodDisruptionBudget>> {
        self.inner
            .pdbs
            .state()
            .into_iter()
            .filter(|x| x.metadata.namespace.as_deref() == Some(ns))
            .collect()
    }

    pub fn target_group_bindings(&self, ns: &str) -> Vec<Arc<TargetGroupBinding>> {
        self.inner
            .tgbs
            .state()
            .into_iter()
            .filter(|x| x.metadata.namespace.as_deref() == Some(ns))
            .collect()
    }
}
