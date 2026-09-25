//! Generic parameterized CRUD builders: a port of the `buildParam*` half of
//! `src/lib/db/crud-helpers.ts` (Postgres, MySQL, SQLite).
//!
//! Values travel as bind parameters; the SQL only carries placeholders. The
//! builders are parameterized exactly as in TypeScript: by an identifier quote
//! fn, an optional cast map, and a placeholder fn. The inline builders
//! (MSSQL/DuckDB) move in phase 2.
//!
//! One intended difference (bug fix 6): the cast map also wraps the
//! primary-key placeholders in the WHERE clause, and update, set-default and
//! delete all take it. TypeScript cast only the value placeholders.

use seaquel_types::{SqlWithBindings, Value};

use crate::dialect::{CastMap, RowValues};

/// Quotes one identifier.
pub type QuoteIdFn<'a> = &'a dyn Fn(&str) -> String;

/// The placeholder for the Nth bind parameter (1-indexed).
pub type PlaceholderFn<'a> = &'a dyn Fn(usize) -> String;

/// `$N` (Postgres, SQLite): the TypeScript default.
pub fn dollar_placeholder(index: usize) -> String {
    format!("${index}")
}

/// `?` (MySQL).
pub fn question_placeholder(_index: usize) -> String {
    "?".to_string()
}

/// `row[pk]`. A missing key is `undefined` in TypeScript, which binds as NULL.
/// A duplicated key resolves to its last entry, as a JS object would.
fn lookup(row: &RowValues, key: &str) -> Value {
    row.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or(Value::Null)
}

/// The placeholder, wrapped in `CAST(… AS type)` when `casts` has a
/// non-empty type for `column`. The type is interpolated verbatim: cast maps
/// are trusted input from the engine's catalog (see [`CastMap`]).
pub fn get_cast_placeholder(
    param_index: usize,
    column: &str,
    placeholder: PlaceholderFn,
    casts: Option<&CastMap>,
) -> String {
    let p = placeholder(param_index);
    match casts.and_then(|c| c.get(column)).filter(|t| !t.is_empty()) {
        Some(ty) => format!("CAST({p} AS {ty})"),
        None => p,
    }
}

/// `pk = $n AND …`. Bug fix 6: a primary key with a type in `casts` is
/// compared as `pk = CAST($n AS type)`, since the UI sends uuid, date and
/// timestamp keys as text and Postgres has no `uuid = text`. The TypeScript
/// builders never cast keys; without a cast the output is unchanged.
fn where_conditions(
    pks: &[String],
    first_index: usize,
    qi: QuoteIdFn,
    casts: Option<&CastMap>,
    placeholder: PlaceholderFn,
) -> String {
    pks.iter()
        .enumerate()
        .map(|(i, pk)| {
            format!(
                "{} = {}",
                qi(pk),
                get_cast_placeholder(i + first_index, pk, placeholder, casts)
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

#[allow(clippy::too_many_arguments)]
pub fn build_param_update(
    schema: &str,
    table: &str,
    column: &str,
    value: Value,
    pks: &[String],
    row: &RowValues,
    qi: QuoteIdFn,
    casts: Option<&CastMap>,
    placeholder: PlaceholderFn,
) -> SqlWithBindings {
    let value_placeholder = get_cast_placeholder(1, column, placeholder, casts);
    let sql = format!(
        "UPDATE {}.{} SET {} = {} WHERE {}",
        qi(schema),
        qi(table),
        qi(column),
        value_placeholder,
        where_conditions(pks, 2, qi, casts, placeholder)
    );
    let mut binds = Vec::with_capacity(pks.len() + 1);
    binds.push(value);
    binds.extend(pks.iter().map(|pk| lookup(row, pk)));
    SqlWithBindings {
        sql,
        bind_values: Some(binds),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_param_set_default(
    schema: &str,
    table: &str,
    column: &str,
    pks: &[String],
    row: &RowValues,
    qi: QuoteIdFn,
    casts: Option<&CastMap>,
    placeholder: PlaceholderFn,
) -> SqlWithBindings {
    let sql = format!(
        "UPDATE {}.{} SET {} = DEFAULT WHERE {}",
        qi(schema),
        qi(table),
        qi(column),
        where_conditions(pks, 1, qi, casts, placeholder)
    );
    SqlWithBindings {
        sql,
        bind_values: Some(pks.iter().map(|pk| lookup(row, pk)).collect()),
    }
}

/// Columns and binds keep the order of `values`.
pub fn build_param_insert(
    schema: &str,
    table: &str,
    values: &[(String, Value)],
    qi: QuoteIdFn,
    casts: Option<&CastMap>,
    placeholder: PlaceholderFn,
) -> SqlWithBindings {
    let column_names: Vec<String> = values.iter().map(|(c, _)| qi(c)).collect();
    let placeholders: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(i, (c, _))| get_cast_placeholder(i + 1, c, placeholder, casts))
        .collect();
    let sql = format!(
        "INSERT INTO {}.{} ({}) VALUES ({})",
        qi(schema),
        qi(table),
        column_names.join(", "),
        placeholders.join(", ")
    );
    SqlWithBindings {
        sql,
        bind_values: Some(values.iter().map(|(_, v)| v.clone()).collect()),
    }
}

pub fn build_param_delete(
    schema: &str,
    table: &str,
    pks: &[String],
    row: &RowValues,
    qi: QuoteIdFn,
    casts: Option<&CastMap>,
    placeholder: PlaceholderFn,
) -> SqlWithBindings {
    let sql = format!(
        "DELETE FROM {}.{} WHERE {}",
        qi(schema),
        qi(table),
        where_conditions(pks, 1, qi, casts, placeholder)
    );
    SqlWithBindings {
        sql,
        bind_values: Some(pks.iter().map(|pk| lookup(row, pk)).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The TypeScript Postgres `qi`: escapes `"` as `""`.
    fn qi(id: &str) -> String {
        format!("\"{}\"", id.replace('"', "\"\""))
    }

    fn backtick(id: &str) -> String {
        format!("`{}`", id.replace('`', "``"))
    }

    fn s(v: &str) -> Value {
        Value::Text(v.into())
    }

    fn row(entries: &[(&str, Value)]) -> RowValues {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn pks(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn casts(entries: &[(&str, &str)]) -> CastMap {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn check(got: SqlWithBindings, sql: &str, binds: Vec<Value>) {
        assert_eq!(got.sql, sql);
        assert_eq!(got.bind_values, Some(binds));
    }

    // crud.json "update, identifiers containing double quotes"
    #[test]
    fn update_quoted_identifiers_with_cast_fixture() {
        let got = build_param_update(
            "my\"schema",
            "the \"table\"",
            "col\"umn",
            s("x"),
            &pks(&["p\"k", "id"]),
            &row(&[("p\"k", s("a")), ("id", Value::Int(2))]),
            &qi,
            Some(&casts(&[("col\"umn", "text")])),
            &dollar_placeholder,
        );
        check(
            got,
            "UPDATE \"my\"\"schema\".\"the \"\"table\"\"\" SET \"col\"\"umn\" = CAST($1 AS text) WHERE \"p\"\"k\" = $2 AND \"id\" = $3",
            vec![s("x"), s("a"), Value::Int(2)],
        );
    }

    // crud.json "update, cast lookup misses the column" / "is an empty map"
    #[test]
    fn update_cast_miss_and_empty_type() {
        let r = row(&[("id", Value::Int(7))]);
        for c in [
            casts(&[("price", "numeric")]),
            casts(&[]),
            casts(&[("name", "")]),
        ] {
            let got = build_param_update(
                "public",
                "products",
                "name",
                s("Widget"),
                &pks(&["id"]),
                &r,
                &qi,
                Some(&c),
                &dollar_placeholder,
            );
            check(
                got,
                "UPDATE \"public\".\"products\" SET \"name\" = $1 WHERE \"id\" = $2",
                vec![s("Widget"), Value::Int(7)],
            );
        }
    }

    // crud.json "update, object value (json)"
    #[test]
    fn update_json_value_fixture() {
        let v = Value::Json(json!({"key": "value", "nested": {"list": [1, 2, 3]}, "ok": true}));
        let got = build_param_update(
            "public",
            "all_types",
            "col_jsonb",
            v.clone(),
            &pks(&["id"]),
            &row(&[
                ("id", Value::Int(1)),
                ("col_jsonb", Value::Json(json!({"key": "old"}))),
            ]),
            &qi,
            Some(&casts(&[("col_jsonb", "jsonb")])),
            &dollar_placeholder,
        );
        check(
            got,
            "UPDATE \"public\".\"all_types\" SET \"col_jsonb\" = CAST($1 AS jsonb) WHERE \"id\" = $2",
            vec![v, Value::Int(1)],
        );
    }

    // crud.json "update, no primary keys (degenerate WHERE)"
    #[test]
    fn update_without_primary_keys_fixture() {
        let got = build_param_update(
            "public",
            "t",
            "c",
            Value::Int(1),
            &[],
            &row(&[("c", Value::Int(0))]),
            &qi,
            None,
            &dollar_placeholder,
        );
        check(
            got,
            "UPDATE \"public\".\"t\" SET \"c\" = $1 WHERE ",
            vec![Value::Int(1)],
        );
    }

    #[test]
    fn missing_pk_in_row_binds_null() {
        let got = build_param_delete("s", "t", &pks(&["id"]), &row(&[]), &qi, None, &dollar_placeholder);
        check(
            got,
            "DELETE FROM \"s\".\"t\" WHERE \"id\" = $1",
            vec![Value::Null],
        );
    }

    // crud.json "setDefault, identifiers containing double quotes"
    #[test]
    fn set_default_fixture() {
        let got = build_param_set_default(
            "a\"b",
            "c\"\"d",
            "\"e\"",
            &pks(&["\"id\""]),
            &row(&[("\"id\"", Value::Int(1))]),
            &qi,
            None,
            &dollar_placeholder,
        );
        check(
            got,
            "UPDATE \"a\"\"b\".\"c\"\"\"\"d\" SET \"\"\"e\"\"\" = DEFAULT WHERE \"\"\"id\"\"\" = $1",
            vec![Value::Int(1)],
        );
    }

    // crud.json "insert, identifiers containing double quotes"
    #[test]
    fn insert_with_cast_fixture() {
        let values = row(&[("a\"b", Value::Int(1)), ("plain", s("x"))]);
        let got = build_param_insert(
            "we\"ird",
            "ta\"ble",
            &values,
            &qi,
            Some(&casts(&[("a\"b", "integer")])),
            &dollar_placeholder,
        );
        check(
            got,
            "INSERT INTO \"we\"\"ird\".\"ta\"\"ble\" (\"a\"\"b\", \"plain\") VALUES (CAST($1 AS integer), $2)",
            vec![Value::Int(1), s("x")],
        );
    }

    // crud.json "insert, no values (degenerate)"
    #[test]
    fn insert_without_values_fixture() {
        let got = build_param_insert("public", "t", &[], &qi, None, &dollar_placeholder);
        check(got, "INSERT INTO \"public\".\"t\" () VALUES ()", vec![]);
    }

    #[test]
    fn insert_keeps_value_order() {
        let values = row(&[
            ("z", Value::Int(1)),
            ("a", Value::Bool(true)),
            ("m", Value::Null),
        ]);
        let got = build_param_insert("s", "t", &values, &qi, None, &dollar_placeholder);
        check(
            got,
            "INSERT INTO \"s\".\"t\" (\"z\", \"a\", \"m\") VALUES ($1, $2, $3)",
            vec![Value::Int(1), Value::Bool(true), Value::Null],
        );
    }

    // crud.json "delete, string and boolean pk values" shape, composite pk
    #[test]
    fn delete_composite_pk() {
        let got = build_param_delete(
            "s\"",
            "\"t",
            &pks(&["k\"1", "k\"2"]),
            &row(&[("k\"1", Value::Int(1)), ("k\"2", s("two"))]),
            &qi,
            None,
            &dollar_placeholder,
        );
        check(
            got,
            "DELETE FROM \"s\"\"\".\"\"\"t\" WHERE \"k\"\"1\" = $1 AND \"k\"\"2\" = $2",
            vec![Value::Int(1), s("two")],
        );
    }

    // ── `?` placeholder (MySQL) ──

    #[test]
    fn question_placeholder_everywhere() {
        let r = row(&[("id", Value::Int(5)), ("region", s("eu"))]);
        let keys = pks(&["id", "region"]);
        let c = casts(&[("price", "DECIMAL(10,2)")]);

        check(
            build_param_update(
                "db", "t", "price", s("1.50"), &keys, &r, &backtick, Some(&c), &question_placeholder,
            ),
            "UPDATE `db`.`t` SET `price` = CAST(? AS DECIMAL(10,2)) WHERE `id` = ? AND `region` = ?",
            vec![s("1.50"), Value::Int(5), s("eu")],
        );
        check(
            build_param_set_default(
                "db",
                "t",
                "price",
                &keys,
                &r,
                &backtick,
                Some(&c),
                &question_placeholder,
            ),
            "UPDATE `db`.`t` SET `price` = DEFAULT WHERE `id` = ? AND `region` = ?",
            vec![Value::Int(5), s("eu")],
        );
        check(
            build_param_insert(
                "db",
                "t",
                &row(&[("price", s("2")), ("name", s("n"))]),
                &backtick,
                Some(&c),
                &question_placeholder,
            ),
            "INSERT INTO `db`.`t` (`price`, `name`) VALUES (CAST(? AS DECIMAL(10,2)), ?)",
            vec![s("2"), s("n")],
        );
        check(
            build_param_delete("db", "t", &keys, &r, &backtick, Some(&c), &question_placeholder),
            "DELETE FROM `db`.`t` WHERE `id` = ? AND `region` = ?",
            vec![Value::Int(5), s("eu")],
        );
    }

    // ── Bug fix 6: primary-key placeholders take the cast map too ──

    #[test]
    fn update_casts_primary_keys() {
        let r = row(&[("id", s("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11")), ("day", s("2024-01-02"))]);
        let c = casts(&[("id", "uuid"), ("day", "date"), ("ts", "timestamp without time zone")]);
        check(
            build_param_update(
                "public",
                "t",
                "ts",
                Value::Null,
                &pks(&["id", "day"]),
                &r,
                &qi,
                Some(&c),
                &dollar_placeholder,
            ),
            "UPDATE \"public\".\"t\" SET \"ts\" = CAST($1 AS timestamp without time zone) WHERE \"id\" = CAST($2 AS uuid) AND \"day\" = CAST($3 AS date)",
            vec![Value::Null, s("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"), s("2024-01-02")],
        );
    }

    #[test]
    fn set_default_and_delete_cast_primary_keys() {
        let r = row(&[("id", s("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11")), ("n", Value::Int(3))]);
        // `n` has no cast, and an empty type means none.
        for c in [casts(&[("id", "uuid")]), casts(&[("id", "uuid"), ("n", "")])] {
            check(
                build_param_set_default(
                    "public",
                    "t",
                    "c",
                    &pks(&["id", "n"]),
                    &r,
                    &qi,
                    Some(&c),
                    &dollar_placeholder,
                ),
                "UPDATE \"public\".\"t\" SET \"c\" = DEFAULT WHERE \"id\" = CAST($1 AS uuid) AND \"n\" = $2",
                vec![s("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"), Value::Int(3)],
            );
            check(
                build_param_delete("public", "t", &pks(&["id", "n"]), &r, &qi, Some(&c), &dollar_placeholder),
                "DELETE FROM \"public\".\"t\" WHERE \"id\" = CAST($1 AS uuid) AND \"n\" = $2",
                vec![s("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"), Value::Int(3)],
            );
        }
    }

    #[test]
    fn primary_key_casts_with_question_placeholder() {
        let r = row(&[("id", s("x"))]);
        let c = casts(&[("id", "CHAR(36)")]);
        check(
            build_param_delete("db", "t", &pks(&["id"]), &r, &backtick, Some(&c), &question_placeholder),
            "DELETE FROM `db`.`t` WHERE `id` = CAST(? AS CHAR(36))",
            vec![s("x")],
        );
    }

    #[test]
    fn cast_placeholder() {
        let c = casts(&[("a", "int"), ("b", "")]);
        assert_eq!(
            get_cast_placeholder(3, "a", &dollar_placeholder, Some(&c)),
            "CAST($3 AS int)"
        );
        assert_eq!(
            get_cast_placeholder(3, "b", &dollar_placeholder, Some(&c)),
            "$3"
        );
        assert_eq!(
            get_cast_placeholder(3, "a", &dollar_placeholder, None),
            "$3"
        );
        assert_eq!(
            get_cast_placeholder(3, "a", &question_placeholder, Some(&c)),
            "CAST(? AS int)"
        );
    }
}
