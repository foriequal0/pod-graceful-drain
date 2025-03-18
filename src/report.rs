use k8s_openapi::api::core::v1::{ObjectReference, Pod};
use kube::Resource;
use kube::runtime::events::{Event, EventType, Recorder};
use kube::runtime::reflector::{Lookup, ObjectRef};
use tracing::{Level, debug, error, event_enabled, info, warn};

pub async fn report(
    recorder: &Recorder,
    reference: &ObjectReference,
    type_: EventType,
    action: &str,
    reason: &str,
    note: String,
) {
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
    let _ = recorder.publish(&event, reference).await;
}

pub async fn debug_report_for_ref<K>(
    recorder: &Recorder,
    object_ref: &ObjectRef<K>,
    action: &str,
    reason: &str,
    note: String,
) where
    K: Resource,
    K::DynamicType: Clone,
{
    if !event_enabled!(Level::DEBUG) {
        return;
    }

    let object_reference = ObjectReference::from(object_ref.clone());
    debug!(action, reason, note);
    report(
        recorder,
        &object_reference,
        EventType::Normal,
        action,
        reason,
        note,
    )
    .await;
}

pub async fn debug_report_for(
    recorder: &Recorder,
    pod: &Pod,
    action: &str,
    reason: &str,
    note: String,
) {
    debug_report_for_ref(recorder, &pod.to_object_ref(()), action, reason, note).await;
}

pub async fn err_report_for_ref<K>(
    recorder: &Recorder,
    object_ref: &ObjectRef<K>,
    action: &str,
    reason: &str,
    note: String,
) where
    K: Resource,
    K::DynamicType: Clone,
{
    if !event_enabled!(Level::ERROR) {
        return;
    }

    let object_reference = ObjectReference::from(object_ref.clone());
    error!(action, reason, note);
    report(
        recorder,
        &object_reference,
        EventType::Warning,
        action,
        reason,
        note,
    )
    .await;
}

pub async fn warn_report_for(
    recorder: &Recorder,
    pod: &Pod,
    action: &str,
    reason: &str,
    note: String,
) {
    if !event_enabled!(Level::WARN) {
        return;
    }

    warn!(action, reason, note);
    report(
        recorder,
        &pod.object_ref(&()),
        EventType::Warning,
        action,
        reason,
        note,
    )
    .await;
}

pub async fn report_for_ref<K>(
    recorder: &Recorder,
    object_ref: &ObjectRef<K>,
    action: &str,
    reason: &str,
    note: String,
) where
    K: Resource,
    K::DynamicType: Clone,
{
    if !event_enabled!(Level::INFO) {
        return;
    }

    let object_reference = ObjectReference::from(object_ref.clone());
    info!(action, reason, note);
    report(
        recorder,
        &object_reference,
        EventType::Normal,
        action,
        reason,
        note,
    )
    .await;
}

pub async fn report_for(recorder: &Recorder, pod: &Pod, action: &str, reason: &str, note: String) {
    report_for_ref(recorder, &pod.to_object_ref(()), action, reason, note).await;
}
