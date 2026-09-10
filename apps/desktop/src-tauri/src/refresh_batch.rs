use std::future::Future;
use tokio_util::sync::CancellationToken;

/// Keep per-item failures isolated, but apply all committed changes exactly once.
/// The dirty flag is separate from success: a post-commit side effect can fail.
pub(crate) async fn run<T, R, E, F, Fut, A, Apply>(
    items: impl IntoIterator<Item = T>,
    cancel: &CancellationToken,
    mut refresh: F,
    apply: A,
) -> (Vec<R>, Result<(), E>)
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = (R, bool)>,
    A: FnOnce(bool) -> Apply,
    Apply: Future<Output = Result<(), E>>,
{
    let mut changed = false;
    let mut results = Vec::new();
    for item in items {
        if cancel.is_cancelled() {
            break;
        }
        let (result, committed) = refresh(item).await;
        changed |= committed;
        results.push(result);
    }
    (results, apply(changed).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn partial_failure_does_not_skip_remaining_items_and_applies_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (results, applied) = run(
            0..3,
            &CancellationToken::new(),
            |item| async move {
                (
                    if item == 1 {
                        Err("failed after commit")
                    } else {
                        Ok(item)
                    },
                    true,
                )
            },
            |changed| {
                assert!(changed);
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, ()>(()) }
            },
        )
        .await;
        assert_eq!(results, vec![Ok(0), Err("failed after commit"), Ok(2)]);
        assert!(applied.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_still_applies_previous_commits() {
        let cancel = CancellationToken::new();
        let (results, applied) = run(
            0..3,
            &cancel,
            |item| {
                cancel.cancel();
                async move { (item, true) }
            },
            |changed| async move {
                assert!(changed);
                Err("core unavailable")
            },
        )
        .await;
        assert_eq!(results, vec![0]);
        assert_eq!(applied, Err("core unavailable"));
    }

    #[tokio::test]
    async fn no_commits_does_not_request_a_rebuild() {
        let (results, _) = run(
            [1, 2],
            &CancellationToken::new(),
            |item| async move { (item, false) },
            |changed| async move {
                assert!(!changed);
                Ok::<_, ()>(())
            },
        )
        .await;
        assert_eq!(results, vec![1, 2]);
    }
}
