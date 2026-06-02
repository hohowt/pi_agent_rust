use pi::agent_cx::AgentCx;
use pi::runtime::RuntimeBuilder;
use std::sync::Arc;
use tokio::sync::Mutex;

#[test]
fn tokio_runtime_drives_spawn_mutex_and_agent_cx_capabilities() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("runtime").join("state.txt");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let state = Arc::new(Mutex::new(Vec::<String>::new()));
        let worker_state = Arc::clone(&state);
        let worker_path = path.clone();
        let join = tokio::spawn(async move {
            let cx = AgentCx::for_request();
            let parent = worker_path.parent().expect("parent");
            cx.fs().create_dir_all(parent).await.expect("create dir");
            cx.fs()
                .write(&worker_path, b"tokio root runtime")
                .await
                .expect("write state");
            cx.time().sleep(std::time::Duration::from_millis(5)).await;
            worker_state.lock().await.push("done".to_string());
        });

        join.await.expect("worker task");
        let bytes = AgentCx::for_request()
            .fs()
            .read(&path)
            .await
            .expect("read state");
        assert_eq!(bytes, b"tokio root runtime");
        assert_eq!(state.lock().await.as_slice(), ["done"]);
    });
}

#[test]
fn pi_runtime_can_be_dropped_inside_tokio_async_context() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let nested = RuntimeBuilder::current_thread()
            .build()
            .expect("nested pi runtime");
        drop(nested);
    });
}
