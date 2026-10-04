//! Probe F3, end to end: the mouse as a terminal sends it (SGR reporting,
//! `CSI < b ; x ; y M`, which the TUI turns on with `?1006h`) reaches the
//! rows, the wheel and the tabs. The TUI runs on a pty of its own over a
//! SQLite connection; what a click did shows on the screen through a key
//! that acts on the selection (Enter opens the selected table, whose name
//! then heads the main view).

#![cfg(unix)]

use std::time::Duration;

use ratatui::layout::Rect;
use seaquel_tui::state::app::Panel;
use seaquel_tui::view::layout;

mod pty;
use pty::{Pty, ANSWER, QUERY};

/// The terminal's size: wide enough for the wide layout.
const SIZE: (u16, u16) = (100, 30);

fn origin() -> seaquel_core::WriteOrigin {
    seaquel_core::WriteOrigin::new(Some("app-window"))
}

/// A data dir with a project `Shop` and a SQLite connection `shop` over a
/// file with tables `a_one`, `b_two`, `c_three` and a view `v_four`.
fn seed(dir: &std::path::Path) {
    use seaquel_core::domain::library::{ConnectionDraft, ProjectDraft, SecretChanges};
    let file = dir.join("shop.db").to_string_lossy().into_owned();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let core = seaquel_terminal::core_builder(seaquel_terminal::CoreOptions::default()).build();
        let ws = core
            .open_workspace(seaquel_core::WorkspaceSpec::new(dir))
            .await
            .unwrap();
        let project: ProjectDraft =
            serde_json::from_value(serde_json::json!({"name": "Shop"})).unwrap();
        let project = ws
            .create_project(&core, &origin(), project)
            .await
            .unwrap()
            .value
            .id;
        let draft: ConnectionDraft = serde_json::from_value(serde_json::json!({
            "projectId": project, "name": "shop", "type": "sqlite",
            "databaseName": file, "host": "", "port": 0, "username": "",
            "savePassword": false, "saveSshPassword": false,
            "saveSshKeyPassphrase": false, "labelIds": [],
        }))
        .unwrap();
        let conn = ws
            .create_connection(&core, &origin(), draft, SecretChanges::default())
            .await
            .unwrap()
            .value
            .id;
        let id = ws
            .connect(
                &core,
                seaquel_core::ConnectRequest::saved(&conn).with_create_if_missing(true),
            )
            .await
            .unwrap();
        for sql in [
            "CREATE TABLE a_one (id INTEGER PRIMARY KEY)",
            "CREATE TABLE b_two (id INTEGER PRIMARY KEY)",
            "CREATE TABLE c_three (id INTEGER PRIMARY KEY)",
            "CREATE VIEW v_four AS SELECT * FROM a_one",
        ] {
            ws.execute(&core, &id, sql, Vec::new()).await.unwrap();
        }
        ws.close_all(&core).await;
        ws.close().await;
    });
}

/// SGR: a left press and its release at a 0-based cell.
fn click(pty: &Pty, x: u16, y: u16) {
    pty.send(format!("\x1b[<0;{};{}M", x + 1, y + 1).as_bytes());
    pty.send(format!("\x1b[<0;{};{}m", x + 1, y + 1).as_bytes());
    settle();
}

/// SGR: one wheel step down (button 65) or up (64).
fn wheel(pty: &Pty, down: bool, x: u16, y: u16) {
    let button = if down { 65 } else { 64 };
    pty.send(format!("\x1b[<{button};{};{}M", x + 1, y + 1).as_bytes());
    settle();
}

/// Two frames, so the next key isn't read in the same batch.
fn settle() {
    std::thread::sleep(Duration::from_millis(60));
}

#[test]
fn sgr_clicks_select_rows_switch_tabs_and_the_wheel_scrolls() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path());
    let pty = Pty::start_sized(
        data.path(),
        &["--project", "Shop", "--connection", "shop"],
        SIZE,
    );
    pty.wait_for(QUERY);
    pty.send(ANSWER);
    pty.wait_for_screen("c_three");
    let areas = layout::areas(Rect::new(0, 0, SIZE.0, SIZE.1), Panel::Tables).unwrap();
    // Panel 2: the `main` schema's row, then its tables.
    let row = |i: u16| areas.tables.y + 1 + i;

    // A click on `c_three`, then Enter opens it.
    click(&pty, areas.tables.x + 4, row(3));
    pty.send(b"\r");
    pty.wait_for_screen("main.c_three");

    // The wheel over panel 2 (the focus is on the main view now): from
    // `c_three` up two rows to `a_one`; then panel 2 and Enter.
    wheel(&pty, false, areas.tables.x + 4, row(2));
    wheel(&pty, false, areas.tables.x + 4, row(2));
    pty.send(b"2");
    settle();
    pty.send(b"\r");
    pty.wait_for_screen("main.a_one");

    // A click on panel 2's `Views` tab shows the view.
    let views_x = areas.tables.x + 1 + "[2]-Tables - ".len() as u16 + 1;
    click(&pty, views_x, areas.tables.y);
    pty.wait_for_screen("v_four");

    // The help says how to select text.
    pty.send(b"?");
    pty.wait_for_screen("Keybindings");
    for _ in 0..200 {
        pty.send(b"j");
    }
    pty.wait_for_screen("shift+drag");
}
