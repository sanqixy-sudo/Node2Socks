use std::{collections::VecDeque, future::Future};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// A selector belongs to one worker until its request finishes. Refill that
/// worker immediately; slow nodes never hold up the other selectors.
pub(crate) async fn run<T, R, F, Fut, Report>(
    items: Vec<T>,
    concurrency: usize,
    cancel: &CancellationToken,
    mut probe: F,
    mut report: Report,
) where
    T: Send + 'static,
    R: Send + 'static,
    F: FnMut(usize, T) -> Fut,
    Fut: Future<Output = R> + Send + 'static,
    Report: FnMut(R),
{
    let mut queue: VecDeque<_> = items.into();
    let mut tasks = JoinSet::new();
    for worker in 0..concurrency {
        if cancel.is_cancelled() {
            break;
        }
        if let Some(item) = queue.pop_front() {
            let future = probe(worker, item);
            tasks.spawn(async move { (worker, future.await) });
        }
    }
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => { tasks.abort_all(); break; }
            joined = tasks.join_next() => {
                match joined {
                    Some(Ok((worker, result))) => {
                        report(result);
                        if !cancel.is_cancelled() {
                            if let Some(item) = queue.pop_front() {
                                let future = probe(worker, item);
                                tasks.spawn(async move { (worker, future.await) });
                            }
                        }
                    }
                    Some(Err(_)) => { tasks.abort_all(); break; }
                    None => break,
                }
            }
        }
    }
    // Drain before the caller releases ownership of the shared selectors.
    while tasks.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Notify;

    #[tokio::test]
    async fn refills_while_a_slow_worker_is_still_running() {
        let unblock = Arc::new(Notify::new());
        let active = Arc::new(AtomicUsize::new(0));
        let mut results = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run(
                (0..12).collect(),
                6,
                &CancellationToken::new(),
                |worker, item| {
                    let unblock = unblock.clone();
                    let active = active.clone();
                    async move {
                        assert!(active.fetch_add(1, Ordering::SeqCst) < 6);
                        if item == 0 {
                            unblock.notified().await;
                        }
                        if item == 6 {
                            assert_ne!(worker, 0);
                            unblock.notify_one();
                        }
                        tokio::task::yield_now().await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        item
                    }
                },
                |result| results.push(result),
            ),
        )
        .await
        .unwrap();
        results.sort();
        assert_eq!(results, (0..12).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn cancellation_stops_refilling_and_drains_workers() {
        let cancel = CancellationToken::new();
        let mut count = 0;
        run(
            (0..100).collect(),
            6,
            &cancel,
            |_, item| async move { item },
            |_| {
                count += 1;
                cancel.cancel();
            },
        )
        .await;
        assert_eq!(count, 1);
        let mut started = false;
        run(
            vec![1],
            6,
            &cancel,
            |_, item| {
                started = true;
                async move { item }
            },
            |_| {},
        )
        .await;
        assert!(!started);
    }
}
