use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Emitted whenever background work (scheduler refresh, Core crash recovery,
/// cloud sync) changes state that the UI renders. The payload scope mirrors
/// the frontend ReloadScope concept: dashboard | nodes | settings | cloud | all.
pub(crate) const SNAPSHOT_DIRTY_EVENT: &str = "n2s://snapshot-dirty";

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SnapshotDirty {
    pub scope: &'static str,
}

/// Best-effort event emission: a failure must never block the caller.
pub(crate) fn emit_snapshot_dirty(app: &AppHandle, scope: &'static str) {
    if let Err(error) = app.emit(SNAPSHOT_DIRTY_EVENT, SnapshotDirty { scope }) {
        tracing::warn!(%error, scope, "failed to emit snapshot-dirty event");
    }
}
