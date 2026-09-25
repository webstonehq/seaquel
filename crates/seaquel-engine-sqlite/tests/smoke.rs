use seaquel_engine_testkit::{run_smoke, SmokeSpec};

#[tokio::test]
async fn smoke() {
    let path = std::env::temp_dir().join(format!("seaquel-smoke-{}.sqlite", uuid::Uuid::new_v4()));
    let config = serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": format!("sqlite:{}", path.display()),
        "create_if_missing": true
    }))
    .unwrap();

    run_smoke(&*seaquel_engine_sqlite::engine(), &config, &SmokeSpec::QUESTION_MARK).await;

    let _ = std::fs::remove_file(&path);
}
