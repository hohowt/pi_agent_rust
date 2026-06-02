use pi::auth::{AuthCredential, AuthStorage};

#[test]
fn auth_storage_async_load_mutate_save_round_trip() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("auth.json");
    std::fs::write(
        &path,
        r#"{
  "openai": {
    "type": "api_key",
    "key": "sk-old"
  }
}"#,
    )
    .expect("write auth file");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut storage = rt
        .block_on(AuthStorage::load_async(path.clone()))
        .expect("load async");
    assert_eq!(storage.api_key("openai").as_deref(), Some("sk-old"));

    storage.set(
        "openai",
        AuthCredential::ApiKey {
            key: "sk-new".to_string(),
        },
    );
    storage.set(
        "anthropic",
        AuthCredential::ApiKey {
            key: "sk-ant".to_string(),
        },
    );
    rt.block_on(storage.save_async()).expect("save async");

    let reloaded = AuthStorage::load(path).expect("reload auth storage");
    assert_eq!(reloaded.api_key("openai").as_deref(), Some("sk-new"));
    assert_eq!(reloaded.api_key("anthropic").as_deref(), Some("sk-ant"));
}
