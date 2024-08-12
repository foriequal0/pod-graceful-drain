use k8s_openapi::api::core::v1::{ObjectReference, Pod};
use kube::runtime::events::{Event, EventType, Recorder};
use kube::Resource;
use tracing::{debug, enabled, info, Level};

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
        reference,
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

    // ignore the error of diagnostic events
    let _ = recorder
        .publish(Event {
            type_,
            action: action.to_string(),
            reason: reason.to_string(),
            note: Some(note),
            secondary: None,
        })
        .await;
}

pub async fn debug_report_for_ref(
    state: &AppState,
    object_ref: ObjectReference,
    action: &str,
    reason: &str,
    note: String,
) {
    if !enabled!(Level::DEBUG) {
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
    if !enabled!(Level::WARN) {
        return;
    }

    info!(action, reason, note);
    report(state, object_ref, EventType::Warning, action, reason, note).await;
}

pub async fn report_for(state: &AppState, pod: &Pod, action: &str, reason: &str, note: String) {
    if !enabled!(Level::INFO) {
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
