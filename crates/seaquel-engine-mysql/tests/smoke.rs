use seaquel_engine_testkit::{config_from_env, run_smoke, SmokeSpec};

/// SEAQUEL_TEST_MYSQL, e.g.
/// {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}
#[tokio::test]
async fn mysql() {
    let Some(config) = config_from_env("SEAQUEL_TEST_MYSQL") else {
        return;
    };
    run_smoke(&*seaquel_engine_mysql::engine(), &config, &SmokeSpec::QUESTION_MARK).await;
}

/// SEAQUEL_TEST_MARIADB. MariaDB uses the mysql driver, e.g.
/// {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3307/seaquel_test"}
#[tokio::test]
async fn mariadb() {
    let Some(config) = config_from_env("SEAQUEL_TEST_MARIADB") else {
        return;
    };
    run_smoke(&*seaquel_engine_mysql::engine(), &config, &SmokeSpec::QUESTION_MARK).await;
}
