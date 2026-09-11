use super::{try_map_option, try_map_sequence};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[tokio::test]
async fn option_none_bypasses_operation() {
    let calls = Arc::new(Mutex::new(0_u8));
    let observed = try_map_option(None::<u8>, {
        let calls = Arc::clone(&calls);
        move |value| {
            *calls.lock().expect("calls mutex should not be poisoned") += 1;
            async move { Ok(value + 1) }
        }
    })
    .await
    .expect("none should be a successful no-op");

    assert_eq!(observed, None);
    assert_eq!(
        *calls.lock().expect("calls mutex should not be poisoned"),
        0
    );
}

#[tokio::test]
async fn sequence_preserves_order_and_stops_at_first_error() {
    let visited = Arc::new(Mutex::new(Vec::new()));
    let result = try_map_sequence(vec![1_u8, 2, 3, 4], {
        let visited = Arc::clone(&visited);
        move |value| {
            let visited = Arc::clone(&visited);
            async move {
                visited
                    .lock()
                    .expect("visited mutex should not be poisoned")
                    .push(value);
                if value == 3 {
                    Err(anyhow::anyhow!("stop"))
                } else {
                    Ok(value * 2)
                }
            }
        }
    })
    .await;

    assert!(result.is_err());
    assert_eq!(
        *visited
            .lock()
            .expect("visited mutex should not be poisoned"),
        vec![1, 2, 3]
    );
}

#[tokio::test]
async fn sequence_accepts_borrowed_exact_size_iterators() {
    let source = vec![3_u8, 1, 4];
    let observed = try_map_sequence(source.iter(), |value| async move { Ok(*value + 1) })
        .await
        .expect("borrowed values should map");

    assert_eq!(observed, vec![4, 2, 5]);
    assert_eq!(source, vec![3, 1, 4]);
}

#[tokio::test]
async fn nested_option_and_sequence_composition_preserves_wrappers() {
    let source = Some(vec![vec![1_u8, 2], vec![3]]);
    let observed = try_map_option(source, |groups| async move {
        try_map_sequence(groups, |values| async move {
            try_map_sequence(values, |value| async move { Ok(value + 10) }).await
        })
        .await
    })
    .await
    .expect("nested composition should map");

    assert_eq!(observed, Some(vec![vec![11, 12], vec![13]]));
}

#[tokio::test]
async fn dropping_pending_sequence_does_not_start_later_items() {
    let visited = Arc::new(Mutex::new(Vec::new()));
    let (started_sender, started_receiver) = oneshot::channel::<()>();
    let (release_sender, release_receiver) = oneshot::channel::<()>();
    let started_sender = Arc::new(Mutex::new(Some(started_sender)));
    let release_receiver = Arc::new(Mutex::new(Some(release_receiver)));

    let task = tokio::spawn({
        let visited = Arc::clone(&visited);
        let started_sender = Arc::clone(&started_sender);
        let release_receiver = Arc::clone(&release_receiver);
        async move {
            try_map_sequence(vec![1_u8, 2], move |value| {
                let visited = Arc::clone(&visited);
                let started_sender = Arc::clone(&started_sender);
                let release_receiver = Arc::clone(&release_receiver);
                async move {
                    visited
                        .lock()
                        .expect("visited mutex should not be poisoned")
                        .push(value);
                    if value == 1 {
                        let sender = started_sender
                            .lock()
                            .expect("started mutex should not be poisoned")
                            .take();
                        if let Some(sender) = sender {
                            let _ = sender.send(());
                        }
                        let receiver = release_receiver
                            .lock()
                            .expect("release mutex should not be poisoned")
                            .take();
                        if let Some(receiver) = receiver {
                            let _ = receiver.await;
                        }
                    }
                    Ok(value)
                }
            })
            .await
        }
    });

    started_receiver
        .await
        .expect("first operation should start");
    task.abort();
    let _ = task.await;

    assert_eq!(
        *visited
            .lock()
            .expect("visited mutex should not be poisoned"),
        vec![1]
    );
    let _ = release_sender.send(());
}
