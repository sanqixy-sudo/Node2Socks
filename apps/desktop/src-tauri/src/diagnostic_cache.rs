//! System process/adapter scans are expensive; UI snapshots never wait for them.
use node2socks_diagnostics::{CoexistenceReport, inspect_windows};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::AppHandle;

#[derive(Default)]
struct Cache {
    report: Option<CoexistenceReport>,
    checked: Option<Instant>,
    running: bool,
}

impl Cache {
    fn begin(&mut self, force: bool) -> bool {
        if self.running
            || (!force
                && self
                    .checked
                    .is_some_and(|time| time.elapsed() < Duration::from_secs(60)))
        {
            return false;
        }
        self.running = true;
        true
    }
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    report: None,
    checked: None,
    running: false,
});

pub(crate) fn snapshot(app: Option<AppHandle>, force: bool) -> Option<CoexistenceReport> {
    let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    let report = cache.report.clone();
    if cache.begin(force) {
        tauri::async_runtime::spawn(async move {
            let result = tauri::async_runtime::spawn_blocking(inspect_windows).await;
            {
                let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
                if let Ok(Ok(report)) = result {
                    cache.report = Some(report);
                }
                cache.checked = Some(Instant::now());
                cache.running = false;
            }
            if let Some(app) = app {
                crate::events::emit_snapshot_dirty(&app, "dashboard");
            }
        });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coalesces_scans_and_honors_expiry_and_explicit_refresh() {
        let mut cache = Cache::default();
        assert!(cache.begin(false));
        assert!(!cache.begin(true));
        cache.running = false;
        cache.checked = Some(Instant::now());
        assert!(!cache.begin(false));
        assert!(cache.begin(true));
        cache.running = false;
        cache.checked = Some(Instant::now() - Duration::from_secs(61));
        assert!(cache.begin(false));
    }
}
