use seaquel_engine_testkit::{config_from_env, run_smoke, SmokeSpec};

/// SEAQUEL_TEST_MSSQL, e.g.
/// {"driver":"mssql","host":"127.0.0.1","port":1433,"username":"sa",
///  "password":"Seaquel_Test_123!","encrypt":true,"trust_cert":true}
#[tokio::test]
async fn smoke() {
    let Some(config) = config_from_env("SEAQUEL_TEST_MSSQL") else {
        return;
    };
    run_smoke(&*seaquel_engine_mssql::engine(), &config, &SmokeSpec::AT_P).await;
}
