use k8s_openapi::api::core::v1::{ObjectReference, Pod};
use kube::Resource;
use kube::runtime::events::{Event, EventType, Recorder};
use tracing::{Level, debug, event_enabled, info, warn};

use crate::webhooks::AppState;
async fn report(
    state: &AppState,
    reference: ObjectReference,
    type_: EventType,
    action: &str,
    reason: &str,
    note: String,
) {
    let recorder = Recorder::new(
        state.api_resolver.client.clone(),
        state.event_reporter.clone(),
    );

    // max limit of the note is 1KB
    let note = if note.len() > 1024 {
        let mut boundary = 1024 - "...".len();
        loop {
            if note.is_char_boundary(boundary) {
                break format!("{}...", &note[..boundary]);
            }

            boundary -= 1;
        }
    } else {
        note
    };

    let event = Event {
        type_,
        action: action.to_string(),
        reason: reason.to_string(),
        note: Some(note),
        secondary: None,
    };

    // ignore the error of diagnostic events
    let _ = recorder.publish(&event, &reference).await;
}

pub async fn debug_report_for_ref(
    state: &AppState,
    object_ref: ObjectReference,
    action: &str,
    reason: &str,
    note: String,
) {
    if !event_enabled!(Level::DEBUG) {
        return;
    }

    debug!(action, reason, note);
    report(state, object_ref, EventType::Normal, action, reason, note).await;
}

pub async fn debug_report_for(
    state: &AppState,
    pod: &Pod,
    action: &str,
    reason: &str,
    note: String,
) {
    debug_report_for_ref(state, pod.object_ref(&()), action, reason, note).await;
}

pub async fn warn_report_for_ref(
    state: &AppState,
    object_ref: ObjectReference,
    action: &str,
    reason: &str,
    note: String,
) {
    if !event_enabled!(Level::WARN) {
        return;
    }

    warn!(action, reason, note);
    report(state, object_ref, EventType::Warning, action, reason, note).await;
}

pub async fn report_for(state: &AppState, pod: &Pod, action: &str, reason: &str, note: String) {
    if !event_enabled!(Level::INFO) {
        return;
    }

    info!(action, reason, note);
    report(
        state,
        pod.object_ref(&()),
        EventType::Normal,
        action,
        reason,
        note,
    )
    .await;
}
