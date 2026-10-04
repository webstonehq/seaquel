//! Model fixtures.

use crate::state::app::Model;
use crate::view::theme::{Theme, ThemeChoice, ThemeEnv};

/// The dark truecolor theme the design shows.
pub fn theme() -> Theme {
    Theme::pick(
        &ThemeEnv {
            colorterm: Some("truecolor".into()),
            no_color: None,
        },
        ThemeChoice::Dark,
    )
}

/// An empty model at the design's 148 × 42.
pub fn model() -> Model {
    model_sized(148, 42)
}

/// An empty model at `width` × `height`.
pub fn model_sized(width: u16, height: u16) -> Model {
    let mut m = Model::new((width, height), theme());
    // The snapshots word the keychain as macOS does, on every OS.
    m.store = crate::state::text::Store::MacKeychain;
    m
}

/// A library like the design's: project-a holds a Postgres connection
/// with its password saved (`conn-saved`) and one without (`conn-ask`);
/// project-b a SQLite file (`conn-file`), one through an SSH tunnel with
/// both passwords unsaved (`conn-tunnel`) and one through a tunnel with key
/// authentication (`conn-key`).
pub fn library() -> crate::state::panels::Library {
    use crate::state::panels::{ConnItem, Library, ProjectItem, Tunnel, TunnelAuth};
    let conn = |id: &str, project: &str, name: &str, engine: &str| ConnItem {
        id: id.into(),
        project_id: project.into(),
        name: name.into(),
        engine: engine.into(),
        host: "db.internal".into(),
        port: Some(5432),
        database: "app".into(),
        save_password: false,
        save_ssh_password: false,
        save_ssh_key_passphrase: false,
        tunnel: None,
        label_ids: Vec::new(),
        ai: Default::default(),
    };
    let saved = ConnItem {
        save_password: true,
        label_ids: vec!["prod".into()],
        ..conn("conn-saved", "project-a", "prod-analytics", "postgres")
    };
    let file = ConnItem {
        host: String::new(),
        port: None,
        database: "/tmp/local.db".into(),
        ..conn("conn-file", "project-b", "local", "sqlite")
    };
    let tunnel = ConnItem {
        tunnel: Some(Tunnel {
            host: "bastion.example".into(),
            port: 22,
            auth: TunnelAuth::Password,
        }),
        ..conn("conn-tunnel", "project-b", "behind-bastion", "postgres")
    };
    let key = ConnItem {
        save_password: true,
        tunnel: Some(Tunnel {
            host: "bastion.example".into(),
            port: 2222,
            auth: TunnelAuth::Key,
        }),
        ..conn("conn-key", "project-b", "keyed", "postgres")
    };
    Library {
        projects: vec![
            ProjectItem {
                id: "project-a".into(),
                name: "Analytics".into(),
            },
            ProjectItem {
                id: "project-b".into(),
                name: "Billing".into(),
            },
        ],
        connections: vec![
            saved,
            conn("conn-ask", "project-a", "staging", "postgres"),
            file,
            tunnel,
            key,
        ],
        labels: Vec::new(),
        ai_off: false,
    }
}

/// Puts `model` in the dialog `context` names (the connect dialogs), with
/// the fixture library loaded.
pub fn dialog(model: &mut Model, context: crate::state::keymap::BarContext) {
    use crate::state::app::{Attempt, Conn, Modal};
    use crate::state::dialogs::{problem, CallError, Notice, PasswordPrompt, Pending, TrustPrompt};
    use crate::state::keymap::BarContext;
    use crate::state::picker::{Picker, Stage};
    use crate::state::secrets::{Secret, SecretKind};
    model.library = library();
    let pending = Pending {
        connection_id: "conn-ask".into(),
        ..Pending::default()
    };
    match context {
        BarContext::Picker => {
            model.modal = Some(Modal::Picker(Picker {
                stage: Stage::Projects,
                selected: 0,
            }))
        }
        BarContext::Password => {
            model.modal = Some(Modal::Password(PasswordPrompt {
                pending,
                kind: SecretKind::Db,
                input: Secret::new("secret"),
                save: true,
                can_save: true,
                reason: None,
            }))
        }
        BarContext::PasswordNoSave => {
            model.store_unavailable = true;
            model.modal = Some(Modal::Password(PasswordPrompt {
                pending,
                kind: SecretKind::Db,
                input: Secret::new("secret"),
                save: false,
                can_save: false,
                reason: Some(crate::state::text::store_unavailable(model.store)),
            }))
        }
        BarContext::Trust => {
            model.modal = Some(Modal::Trust(TrustPrompt {
                pending,
                host: "bastion.example".into(),
                port: 22,
                fingerprint: "SHA256:k3yF1ngerpr1nt".into(),
            }))
        }
        BarContext::Problem => {
            model.modal = Some(Modal::Problem(problem(
                &CallError::new("HOST_KEY_MISMATCH", "The host key changed."),
                None,
            )))
        }
        BarContext::ProblemRetry => {
            model.modal = Some(Modal::Problem(problem(
                &CallError::new("CONNECTION_ERROR", "password authentication failed"),
                Some((SecretKind::Db, pending)),
            )))
        }
        BarContext::Notice => model.modal = Some(Modal::Notice(Notice("Saved.".into()))),
        BarContext::ProblemReconnect => {
            let mut p = problem(
                &CallError::new(
                    "ENGINE_UNAVAILABLE",
                    "The DuckDB helper didn't start in time.",
                ),
                None,
            );
            p.reconnect = Some(pending);
            model.modal = Some(Modal::Problem(p));
        }
        BarContext::InstallChecking
        | BarContext::InstallAsk
        | BarContext::InstallDownloading
        | BarContext::InstallFailed
        | BarContext::InstallFailedFinal => {
            use crate::state::install::{failure, InstallDialog, Offer, Stage as Install, Step};
            let stage = match context {
                BarContext::InstallChecking => Install::Checking,
                BarContext::InstallAsk => Install::Ask(Offer {
                    size: 11_700_000,
                    repair: false,
                }),
                BarContext::InstallDownloading => Install::Downloading {
                    bytes: 4_212_000,
                    total: 11_700_000,
                },
                BarContext::InstallFailedFinal => Install::Failed(failure(
                    &CallError::new("NOT_SUPPORTED", "no DuckDB download for this platform"),
                    Step::Download,
                )),
                _ => Install::Failed(failure(
                    &CallError::new(
                        "NETWORK_ERROR",
                        "couldn't connect to the release server (or the proxy)",
                    ),
                    Step::Download,
                )),
            };
            model.modal = Some(Modal::InstallDuckdb(InstallDialog {
                pending,
                op: 1,
                stage,
            }));
        }
        BarContext::Keychain => {
            let t0 = std::time::Instant::now();
            model.conn = Conn::Connecting(Attempt {
                attempt: 1,
                pending,
            });
            model.keychain = Some(t0);
            model.now = Some(t0 + std::time::Duration::from_secs(1));
        }
        _ => unreachable!("{context:?}"),
    }
}

/// The design's 1a data as far as Task 3 goes: connected to
/// `prod-analytics`, panel 2 with `public` (the invoices tables) and two
/// folded schemas, panel 3 with the four saved queries and a history.
pub fn connected(width: u16, height: u16) -> Model {
    use crate::state::app::{Conn, Load};
    use crate::state::panels::{HistoryItem, SavedItem, TableItem, TableKind};
    let mut m = model_sized(width, height);
    m.library = library();
    m.project = Some("project-a".into());
    m.conn = Conn::Connected {
        id: "conn-saved".into(),
        core_id: "core-1".into(),
    };
    let table = |schema: &str, name: &str, kind, rows| TableItem {
        schema: schema.into(),
        name: name.into(),
        kind,
        row_count: Some(rows),
        columns: vec![
            ("id".into(), "int8".into()),
            ("customer".into(), "text".into()),
            ("total".into(), "numeric".into()),
        ],
    };
    m.schema = vec![
        table("public", "customers", TableKind::Table, 12_400),
        table("public", "invoices", TableKind::Table, 48_100),
        table("public", "invoice_line_items", TableKind::Table, 312_000),
        table("public", "items", TableKind::Table, 86),
        table("public", "orders", TableKind::Table, 91_700),
        table("public", "payments", TableKind::Table, 40_200),
        table("public", "active_customers", TableKind::View, 0),
        table("public", "mrr_monthly", TableKind::MaterializedView, 0),
        table("analytics", "events", TableKind::Table, 1_300_000),
        table("auth", "users", TableKind::Table, 3),
    ];
    m.schema_load = Load::Loaded;
    m.folded_schemas = ["analytics".to_string(), "auth".to_string()].into();
    let saved = |name: &str, folder: Option<&str>, shared: bool, sql: &str| SavedItem {
        id: format!("saved-{name}"),
        name: name.into(),
        folder: folder.map(Into::into),
        shared,
        sql: sql.into(),
    };
    m.saved_items = vec![
        saved(
            "revenue_by_month.sql",
            None,
            true,
            "SELECT c.name,\n       date_trunc('month', i.issued_at) AS month\nFROM   invoices i;",
        ),
        saved("top_customers.sql", None, true, "SELECT 1;"),
        saved("churn_cohorts.sql", None, false, "SELECT 2;"),
        saved("slow_invoices.sql", Some("perf"), false, "SELECT 3;"),
    ];
    m.history_items = vec![HistoryItem {
        id: "h1".into(),
        when: "12:03:51".into(),
        sql: "SELECT * FROM public.invoices ORDER BY issued_at DESC LIMIT 40;".into(),
        elapsed_ms: 18.0,
        rows: 40.0,
    }];
    m.refresh_lists();
    m.tables.selected = 2;
    m
}

/// `public.invoices`, the design's table.
pub fn invoices() -> seaquel_core::domain::edits::TableTarget {
    seaquel_core::domain::edits::TableTarget {
        schema: "public".into(),
        table: "invoices".into(),
    }
}

/// A column as Core's metadata reports it.
pub fn column(name: &str, ty: &str) -> seaquel_types::SchemaColumn {
    seaquel_types::SchemaColumn {
        name: name.into(),
        ty: ty.into(),
        cast_type: None,
        nullable: true,
        default_value: None,
        is_primary_key: false,
        is_foreign_key: false,
        foreign_key_ref: None,
        collation: None,
        is_unique: false,
        in_unique_constraint: false,
    }
}

/// The design's `invoices` metadata: its columns, keys, defaults and
/// indexes (screen 1a's Structure, Indexes and Constraints tabs), and DDL
/// as Core's Postgres dialect builds it.
pub fn invoices_meta() -> crate::state::browse::TableMeta {
    use seaquel_types::{ForeignKeyRef, SchemaColumn, SchemaIndex};
    let col = |name: &str, ty: &str, nullable: bool, default: Option<&str>| SchemaColumn {
        nullable,
        default_value: default.map(Into::into),
        ..column(name, ty)
    };
    let columns = vec![
        SchemaColumn {
            is_primary_key: true,
            ..col(
                "id",
                "int8",
                false,
                Some("nextval('invoices_id_seq'::regclass)"),
            )
        },
        SchemaColumn {
            is_foreign_key: true,
            foreign_key_ref: Some(ForeignKeyRef {
                referenced_schema: "public".into(),
                referenced_table: "customers".into(),
                referenced_column: "id".into(),
            }),
            ..col("customer", "text", false, None)
        },
        col(
            "status",
            "invoice_status",
            false,
            Some("'draft'::invoice_status"),
        ),
        col("issued_at", "date", false, Some("CURRENT_DATE")),
        col("due_at", "date", true, None),
        col("total", "numeric(12,2)", false, Some("0")),
        col("paid_at", "timestamptz", true, None),
    ];
    let index = |name: &str, cols: &[&str], unique: bool| SchemaIndex {
        name: name.into(),
        columns: cols.iter().map(|c| c.to_string()).collect(),
        unique,
        ty: "btree".into(),
    };
    crate::state::browse::TableMeta {
        columns,
        indexes: vec![
            index("invoices_pkey", &["id"], true),
            index("invoices_issued_at_idx", &["issued_at"], false),
            index("invoices_customer_idx", &["customer"], false),
            index("invoices_status_idx", &["status"], false),
        ],
        ddl: Ok("CREATE TABLE \"public\".\"invoices\" (\n  \"id\" int8 NOT NULL DEFAULT nextval('invoices_id_seq'::regclass),\n  \"customer\" text NOT NULL,\n  \"status\" invoice_status NOT NULL DEFAULT 'draft'::invoice_status,\n  \"issued_at\" date NOT NULL DEFAULT CURRENT_DATE,\n  \"due_at\" date,\n  \"total\" numeric(12,2) NOT NULL DEFAULT 0,\n  \"paid_at\" timestamptz,\n  PRIMARY KEY (\"id\"),\n  FOREIGN KEY (\"customer\") REFERENCES \"public\".\"customers\" (\"id\")\n);\n\nCREATE INDEX \"invoices_issued_at_idx\" ON \"public\".\"invoices\" (\"issued_at\");".into()),
    }
}

/// The design's fifteen `invoices` rows (the prototype's `ROWS`), as page 1
/// of 482 (48,112 rows).
pub fn invoices_page() -> crate::state::grid::Page {
    use seaquel_core::Value;
    /// id, customer, status, issued_at, due_at, total, paid_at.
    type Invoice = (
        i64,
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
        Option<&'static str>,
    );
    const ROWS: [Invoice; 15] = [
        (
            48112,
            "Acme Corp",
            "sent",
            "2026-09-24",
            Some("2026-10-24"),
            "4210.00",
            None,
        ),
        (
            48111,
            "Initech",
            "paid",
            "2026-09-24",
            Some("2026-10-24"),
            "860.50",
            Some("2026-09-25 09:12:00+00"),
        ),
        (
            48110,
            "Umbrella Health",
            "draft",
            "2026-09-23",
            None,
            "12400.00",
            None,
        ),
        (
            48109,
            "Globex Ltd",
            "overdue",
            "2026-08-22",
            Some("2026-09-21"),
            "2975.00",
            None,
        ),
        (
            48108,
            "Stark Industrial",
            "paid",
            "2026-09-21",
            Some("2026-10-21"),
            "1025.00",
            Some("2026-09-22 14:40:00+00"),
        ),
        (
            48107,
            "Wayne Logistics",
            "sent",
            "2026-09-20",
            Some("2026-10-20"),
            "7880.00",
            None,
        ),
        (
            48106,
            "Hooli",
            "overdue",
            "2026-08-18",
            Some("2026-09-17"),
            "540.00",
            None,
        ),
        (
            48105,
            "Soylent Foods",
            "paid",
            "2026-09-18",
            Some("2026-10-18"),
            "2300.00",
            Some("2026-09-19 08:03:00+00"),
        ),
        (
            48104,
            "Vandelay Imports",
            "sent",
            "2026-09-17",
            Some("2026-10-17"),
            "615.75",
            None,
        ),
        (
            48103,
            "Pied Piper",
            "paid",
            "2026-09-16",
            Some("2026-10-16"),
            "9999.00",
            Some("2026-09-16 17:21:00+00"),
        ),
        (
            48102,
            "Cyberdyne",
            "overdue",
            "2026-08-14",
            Some("2026-09-13"),
            "3410.20",
            None,
        ),
        (
            48101,
            "Tyrell Corp",
            "paid",
            "2026-09-14",
            Some("2026-10-14"),
            "18250.00",
            Some("2026-09-15 11:48:00+00"),
        ),
        (
            48100,
            "Massive Dynamic",
            "sent",
            "2026-09-13",
            Some("2026-10-13"),
            "1190.00",
            None,
        ),
        (
            48099,
            "Oscorp",
            "paid",
            "2026-09-12",
            Some("2026-10-12"),
            "4475.00",
            Some("2026-09-12 16:30:00+00"),
        ),
        (
            48098,
            "Aperture Labs",
            "sent",
            "2026-09-11",
            Some("2026-10-11"),
            "730.00",
            None,
        ),
    ];
    let text = |s: &str| Value::Text(s.into());
    let opt = |s: Option<&str>| s.map_or(Value::Null, text);
    crate::state::grid::Page {
        sql:
            "SELECT * FROM \"public\".\"invoices\" ORDER BY \"issued_at\" DESC\nLIMIT 101 OFFSET 0"
                .into(),
        columns: [
            "id",
            "customer",
            "status",
            "issued_at",
            "due_at",
            "total",
            "paid_at",
        ]
        .map(String::from)
        .to_vec(),
        rows: ROWS
            .iter()
            .map(|(id, customer, status, issued, due, total, paid)| {
                vec![
                    Value::Int(*id),
                    text(customer),
                    text(status),
                    text(issued),
                    opt(*due),
                    Value::Decimal((*total).into()),
                    opt(*paid),
                ]
            })
            .collect(),
        page: 1,
        page_size: 100,
        total_rows: 48_112,
        total_pages: 482,
        count_estimated: false,
        elapsed_ms: 18.0,
    }
}

/// Screen 1a: connected, `public.invoices` opened with its page and
/// metadata, the main view focused on the grid.
pub fn browsing(width: u16, height: u16) -> Model {
    use crate::state::app::Panel;
    use crate::state::browse::{Meta, Opened};
    use crate::state::panels::TableKind;
    let mut m = connected(width, height);
    m.tables.selected = 2;
    m.browse.opened = Some(Opened {
        target: invoices(),
        kind: TableKind::Table,
        core_id: "core-1".into(),
    });
    m.browse.meta = Meta::Loaded(invoices_meta());
    m.browse.page = Some(invoices_page());
    m.browse.page_no = 1;
    m.browse.next_op = 1;
    m.focus = Panel::Main;
    m.ctx = Panel::Tables;
    crate::state::browse::refresh(&mut m);
    m
}

/// [`browsing`], with a table that has no primary key.
pub fn keyless(width: u16, height: u16) -> Model {
    use crate::state::browse::Meta;
    let mut m = browsing(width, height);
    if let Meta::Loaded(meta) = &mut m.browse.meta {
        for c in &mut meta.columns {
            c.is_primary_key = false;
        }
    }
    m
}

/// What Core would plan for an edit of the fixture's `invoices` (the SQL
/// is only shown).
pub fn plan_of(
    edit: &seaquel_core::domain::edits::Edit,
) -> seaquel_core::domain::edits::PlannedChange {
    use seaquel_core::domain::edits::{Edit, PlannedChange};
    use seaquel_core::sql::statements::QueryType;
    use seaquel_core::Value;
    let (sql, query_type, params) = match edit {
        Edit::UpdateCell {
            column, value, key, ..
        } => (
            format!("UPDATE \"public\".\"invoices\" SET \"{column}\" = $1 WHERE \"id\" = $2"),
            QueryType::Update,
            vec![value.clone(), key[0].1.clone()],
        ),
        Edit::SetDefault { column, key, .. } => (
            format!("UPDATE \"public\".\"invoices\" SET \"{column}\" = DEFAULT WHERE \"id\" = $1"),
            QueryType::Update,
            vec![key[0].1.clone()],
        ),
        Edit::DeleteRow { key, .. } => (
            "DELETE FROM \"public\".\"invoices\" WHERE \"id\" = $1".to_string(),
            QueryType::Delete,
            vec![key[0].1.clone()],
        ),
        Edit::InsertRow { values, .. } => (
            "INSERT INTO \"public\".\"invoices\" (\"customer\") VALUES ($1)".to_string(),
            QueryType::Insert,
            values.iter().map(|(_, v)| v.clone()).collect(),
        ),
        _ => (String::new(), QueryType::Other, vec![Value::Null]),
    };
    PlannedChange {
        sql,
        params,
        query_type,
        dml: true,
        summary: None,
    }
}

/// Answers every plan in `effects` as Core would.
pub fn answer_plans(model: &mut Model, effects: &[crate::state::app::Effect]) {
    use crate::state::app::{update, Effect, Msg};
    for effect in effects {
        if let Effect::PlanEdit(call) = effect {
            update(
                model,
                Msg::Planned {
                    id: call.request.id.clone(),
                    seq: call.request.seq,
                    result: Ok(plan_of(&call.request.edit)),
                },
            );
        }
    }
}

/// Screen 1c's queue on the fixture's `invoices` (the design's spread over
/// `items` and `invoices` is one table here): the edit of `48109.total`,
/// the delete of `48106`, an insert and an edit of `48108.customer`, all
/// planned. `prod` keeps the connection's `prod` label.
pub fn staged(width: u16, height: u16, prod: bool) -> Model {
    use crate::state::app::update;
    use crate::testing::keys::{key, press};
    use crossterm::event::KeyCode;
    let mut m = browsing(width, height);
    if !prod {
        m.library.connections[0].label_ids.clear();
    }
    let mut effects = Vec::new();
    let mut typed = |m: &mut Model, text: &str| {
        for c in text.chars() {
            effects.extend(update(m, key(c)));
        }
    };
    m.browse.row = 3;
    m.browse.col = 5;
    typed(&mut m, "e");
    for _ in 0..7 {
        update(&mut m, press(KeyCode::Backspace));
    }
    typed(&mut m, "3150.00");
    let mut more = update(&mut m, press(KeyCode::Enter));
    m.browse.row = 6;
    typed(&mut m, "d");
    typed(&mut m, "a");
    m.browse.col = 1;
    typed(&mut m, "eNew Co");
    more.extend(update(&mut m, press(KeyCode::Enter)));
    // 48108, below the insert.
    m.browse.row = 5;
    m.browse.col = 1;
    typed(&mut m, "e!");
    more.extend(update(&mut m, press(KeyCode::Enter)));
    effects.extend(more);
    answer_plans(&mut m, &effects);
    assert_eq!(m.queue.entries().len(), 4);
    assert!(m.queue.all_planned());
    m
}

/// Connected (as [`connected`]) with the query view open on an untitled tab
/// holding `text`, the cursor at its end, in Insert mode. `prod` keeps the
/// connection's predefined `prod` label.
pub fn querying(width: u16, height: u16, text: &str, prod: bool) -> Model {
    use crate::state::editor::Normal;
    let mut m = connected(width, height);
    if !prod {
        m.library.connections[0].label_ids.clear();
    }
    crate::state::query::new_tab(&mut m);
    let tab = m.query.active_mut().unwrap();
    tab.editor.set_text(text);
    tab.editor.normal(Normal::Bottom);
    tab.editor.normal(Normal::LineEnd);
    crate::state::query::sync(&mut m);
    m
}

/// A query tab whose run is in flight (`SELECT 1`).
pub fn running(width: u16, height: u16) -> Model {
    let mut m = querying(width, height, "SELECT 1", false);
    crate::state::query::run(&mut m, crate::state::query::RunKind::All, false);
    assert!(m.running());
    m
}

/// Screen 1b: `revenue_by_month.sql` (saved, then edited) and an untitled
/// tab, the cursor after `i.iss` on line 7 with the completion popup open,
/// and the Explain tab showing the design's ANALYZE plan.
pub fn screen_1b(width: u16, height: u16) -> Model {
    use crate::state::app::Msg;
    use crate::state::editor::Normal;
    use crate::state::query::{self, ExplainView, ResultTab};
    let mut m = connected(width, height);
    if let Some(t) = m
        .schema
        .iter_mut()
        .find(|t| t.schema == "public" && t.name == "invoices")
    {
        t.columns = [
            ("id", "int8"),
            ("customer_id", "int8"),
            ("issued_at", "date"),
            ("issuer_id", "int8"),
            ("is_subscription", "bool"),
            ("total", "numeric"),
        ]
        .iter()
        .map(|(n, ty)| (n.to_string(), ty.to_string()))
        .collect();
    }
    m.focus_panel(crate::state::app::Panel::Saved);
    crate::state::app::update(&mut m, crate::testing::keys::key('o'));
    query::new_tab(&mut m);
    m.query.active = 0;
    let sql = "SELECT c.name,\n       date_trunc('month', i.issued_at) AS month,\n       sum(li.quantity * li.unit_price) AS revenue\nFROM   invoices i\nJOIN   customers c ON c.id = i.customer_id\nJOIN   invoice_line_items li ON li.invoice_id = i.id\nWHERE  i.iss\nGROUP BY 1, 2\nORDER BY revenue DESC;";
    let tab = &mut m.query.tabs[0];
    tab.editor.set_text(sql);
    tab.editor.normal(Normal::G);
    tab.editor.normal(Normal::G);
    for _ in 0..6 {
        tab.editor.normal(Normal::Down);
    }
    tab.editor.normal(Normal::LineEnd);
    tab.result_tab = ResultTab::Explain;
    tab.explain = Some(ExplainView::Loaded(Box::new(
        crate::state::explain::tests::design_plan(),
    )));
    query::complete(&mut m, false);
    crate::state::app::update(&mut m, Msg::Resize(width, height));
    m
}

/// A run of four statements that ended: two SELECTs with rows (the first
/// shown), an UPDATE and one that failed; the results box focused.
pub fn ran(width: u16, height: u16) -> Model {
    use crate::state::app::{update, Msg};
    use crate::state::dialogs::CallError;
    use crate::state::query::{self, Pane, RunKind, RunMsg};
    use seaquel_core::domain::run::{PageSource, StatementKind};
    use seaquel_core::Value;
    let sql = "SELECT id, customer, total FROM invoices;\nSELECT count(*) FROM invoices;\nUPDATE invoices SET total = 0 WHERE id < 3;\nSELEC 1";
    let mut m = querying(width, height, sql, false);
    let effects = query::run(&mut m, RunKind::All, false);
    let Some(crate::state::app::Effect::Run(call)) = effects.last().cloned() else {
        panic!("{effects:?}")
    };
    let start = |index: u32, sql: &str, kind| RunMsg::Start {
        index,
        sql: sql.into(),
        source: PageSource {
            sql: sql.into(),
            params: Vec::new(),
        },
        kind,
        page: 1,
        page_size: 100,
    };
    let done = |index: u32, rows: u64, affected: Option<u64>| RunMsg::Done {
        index,
        elapsed_ms: 4.2,
        total_rows: rows,
        total_pages: 1,
        count_estimated: false,
        rows_affected: affected,
    };
    let events = vec![
        start(
            0,
            "SELECT id, customer, total FROM invoices",
            StatementKind::Page,
        ),
        RunMsg::Batch {
            columns: Some(vec!["id".into(), "customer".into(), "total".into()]),
            rows: vec![
                vec![
                    Value::Int(48112),
                    Value::Text("Acme Corp".into()),
                    Value::Decimal("4210.00".into()),
                ],
                vec![
                    Value::Int(48111),
                    Value::Text("Initech".into()),
                    Value::Null,
                ],
                vec![
                    Value::Int(48110),
                    Value::Text("Globex".into()),
                    Value::Decimal("88.10".into()),
                ],
            ],
        },
        done(0, 3, None),
        start(1, "SELECT count(*) FROM invoices", StatementKind::Page),
        RunMsg::Batch {
            columns: Some(vec!["count".into()]),
            rows: vec![vec![Value::Int(3)]],
        },
        done(1, 1, None),
        start(
            2,
            "UPDATE invoices SET total = 0 WHERE id < 3",
            StatementKind::Write,
        ),
        done(2, 0, Some(2)),
        start(3, "SELEC 1", StatementKind::Utility),
        RunMsg::Failed {
            index: 3,
            error: CallError::new("SYNTAX_ERROR", "syntax error at or near \"SELEC\""),
            elapsed_ms: 0.4,
            sql: None,
        },
        RunMsg::Finished {
            statements: 4,
            succeeded: false,
            history: None,
        },
    ];
    for event in events {
        update(
            &mut m,
            Msg::Run {
                tab: call.tab,
                op: call.op,
                event,
            },
        );
    }
    m.query.pane = Pane::Results;
    query::statement_step(&mut m, false);
    m
}

/// A model in one of the query view's bar contexts (and dialogs).
pub fn query_context(context: crate::state::keymap::BarContext) -> Model {
    use crate::state::app::{update, Modal};
    use crate::state::keymap::BarContext;
    use crate::testing::keys::{ctrl, key, press};
    use crossterm::event::KeyCode;
    let typed = |m: &mut Model, text: &str| {
        for c in text.chars() {
            update(m, key(c));
        }
    };
    match context {
        BarContext::QueryInsert => querying(148, 42, "SELECT 1", false),
        BarContext::QueryNormal | BarContext::QueryCommand => {
            let mut m = querying(148, 42, "SELECT 1", false);
            update(&mut m, press(KeyCode::Esc));
            if context == BarContext::QueryCommand {
                typed(&mut m, ":w");
            }
            m
        }
        BarContext::Completion => {
            let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE i", false);
            typed(&mut m, ".");
            m
        }
        BarContext::Results => ran(148, 42),
        BarContext::Params => {
            let mut m = querying(
                148,
                42,
                "SELECT * FROM invoices WHERE issued_at BETWEEN {{from}} AND {{to}}",
                false,
            );
            update(&mut m, ctrl('r'));
            typed(&mut m, "2026-01-01");
            m
        }
        BarContext::RunConfirm => {
            let mut m = querying(148, 42, "DELETE FROM invoices", true);
            update(&mut m, ctrl('r'));
            typed(&mut m, "pr");
            m
        }
        BarContext::SaveAs => {
            let mut m = querying(148, 42, "SELECT 42", false);
            update(&mut m, ctrl('s'));
            typed(&mut m, "answer");
            m
        }
        BarContext::Cell => {
            let mut m = ran(148, 42);
            m.modal = Some(Modal::Cell(crate::state::query::CellView {
                column: "doc".into(),
                text: serde_json::to_string_pretty(&serde_json::json!({
                    "invoice": 48112, "lines": [1, 2, 3], "note": "paid in full"
                }))
                .unwrap(),
                scroll: 0,
            }));
            m
        }
        other => unreachable!("{other:?}"),
    }
}

/// Screen 1d: Ask AI over an untitled query tab on `prod-analytics`, the
/// design's request (with its two mentions) answered with the design's SQL
/// in 1.8 s by `claude-sonnet`.
pub fn screen_1d(width: u16, height: u16) -> Model {
    use crate::state::app::{update, Msg};
    use crate::testing::keys::{ctrl, key, press};
    use crossterm::event::KeyCode;
    let mut m = querying(width, height, "", false);
    m.library.connections[0].ai.model = Some("claude-sonnet".into());
    update(&mut m, ctrl('k'));
    for c in "top 10 customers by revenue this quarter, only @invoices with status paid, \
              include @customers.country"
        .chars()
    {
        update(&mut m, key(c));
    }
    update(&mut m, press(KeyCode::Esc));
    update(&mut m, press(KeyCode::Enter));
    let op = match &m.modal {
        Some(crate::state::app::Modal::Ask(a)) => a.op,
        other => panic!("{other:?}"),
    };
    update(
        &mut m,
        Msg::Generated {
            op,
            result: Ok(crate::state::query::SqlText(
                "SELECT c.name, c.country, sum(li.quantity * li.unit_price) AS revenue\n\
                 FROM   invoices i\n\
                 JOIN   customers c ON c.id = i.customer_id\n\
                 JOIN   invoice_line_items li ON li.invoice_id = i.id\n\
                 WHERE  i.status = 'paid'\n  \
                 AND  i.issued_at >= date_trunc('quarter', now())\n\
                 GROUP BY c.id, c.name, c.country\n\
                 ORDER BY revenue DESC LIMIT 10;"
                    .into(),
            )),
            elapsed_ms: 1_800,
        },
    );
    m
}

/// The model in Ask AI's stage `context` names (the key bar test).
pub fn ask_context(context: crate::state::keymap::BarContext) -> Model {
    use crate::state::app::update;
    use crate::state::keymap::BarContext;
    use crate::testing::keys::{ctrl, key};
    match context {
        BarContext::AskAnswer => screen_1d(148, 42),
        BarContext::AskDone => {
            let mut m = screen_1d(148, 42);
            m.conn = crate::state::app::Conn::Failed {
                id: "conn-saved".into(),
            };
            update(&mut m, ctrl('r'));
            m
        }
        BarContext::AskPrompt | BarContext::AskMention | BarContext::AskWaiting => {
            let mut m = querying(148, 42, "", false);
            update(&mut m, ctrl('k'));
            for c in "count @inv".chars() {
                update(&mut m, key(c));
            }
            match context {
                BarContext::AskPrompt => {
                    update(&mut m, key(' '));
                }
                BarContext::AskWaiting => {
                    update(&mut m, key(' '));
                    update(
                        &mut m,
                        crate::testing::keys::press(crossterm::event::KeyCode::Enter),
                    );
                }
                _ => {}
            }
            m
        }
        other => unreachable!("{other:?}"),
    }
}
