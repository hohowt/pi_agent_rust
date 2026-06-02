use pi::agent_cx::AgentCx;
use std::time::{Duration, Instant};

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(future)
}

#[test]
fn agent_cx_filesystem_capability_round_trips_bytes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let nested = temp.path().join("nested").join("dir");
    let file = nested.join("message.txt");

    run_async(async {
        let cx = AgentCx::for_request();
        cx.fs()
            .create_dir_all(&nested)
            .await
            .expect("create nested dir");
        cx.fs()
            .write(&file, b"hello from agent cx")
            .await
            .expect("write file");
        let bytes = cx.fs().read(&file).await.expect("read file");
        assert_eq!(bytes, b"hello from agent cx");
    });
}

#[test]
fn agent_cx_time_capability_sleeps_at_least_requested_duration() {
    run_async(async {
        let cx = AgentCx::for_request();
        let started = Instant::now();
        cx.time().sleep(Duration::from_millis(15)).await;
        assert!(
            started.elapsed() >= Duration::from_millis(10),
            "sleep returned too early: {:?}",
            started.elapsed()
        );
    });
}
