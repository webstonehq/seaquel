use seaquel_engine_testkit::{config_from_env, run_smoke, SmokeSpec};

/// Set SEAQUEL_TEST_POSTGRES to a ConnectConfig JSON to run this, e.g.
/// {"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}
#[tokio::test]
async fn smoke() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    run_smoke(&*seaquel_engine_postgres::engine(), &config, &SmokeSpec::DOLLAR).await;
}
