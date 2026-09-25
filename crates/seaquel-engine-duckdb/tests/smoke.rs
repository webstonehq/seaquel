use seaquel_engine_testkit::{run_smoke, SmokeSpec};

#[tokio::test]
async fn smoke() {
    let config = serde_json::from_value(serde_json::json!({
        "driver": "duckdb",
        "path": ":memory:"
    }))
    .unwrap();
    run_smoke(&*seaquel_engine_duckdb::engine(), &config, &SmokeSpec::QUESTION_MARK).await;
}
