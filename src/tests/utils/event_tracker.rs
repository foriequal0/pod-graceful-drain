use std::time::{Duration, Instant};

use crate::CONTROLLER_NAME;
use crate::tests::utils::context::TestContext;
use futures::StreamExt;
use futures::stream::BoxStream;
use k8s_openapi::api::events::v1::Event;
use kube::Api;
use kube::api::{WatchEvent, WatchParams};

pub struct EventTracker {
    stream: BoxStream<'static, kube::Result<WatchEvent<Event>>>,
    timeout: Duration,
}

impl EventTracker {
    pub async fn new(context: &TestContext, timeout: Duration) -> Self {
        let api: Api<Event> =
            Api::namespaced(context.api_resolver.client.clone(), &context.namespace);
        let params = WatchParams {
            field_selector: Some(format!("reportingController={CONTROLLER_NAME}")),
            ..WatchParams::default()
        };
        let stream = api.watch(&params, "0").await.unwrap().boxed();
        Self { stream, timeout }
    }

    pub async fn issued_soon(&mut self, action: &str, reason: &str) -> bool {
        let start = Instant::now();

        while Instant::now() - start < self.timeout {
            let result = tokio::time::timeout_at(
                tokio::time::Instant::from_std(start + self.timeout),
                self.stream.next(),
            )
            .await;

            let Ok(Some(Ok(watch_event))) = result else {
                break;
            };

            let WatchEvent::Added(event) = watch_event else {
                continue;
            };

            if event.action.as_deref() == Some(action) && event.reason.as_deref() == Some(reason) {
                return true;
            }
        }

        false
    }
}
