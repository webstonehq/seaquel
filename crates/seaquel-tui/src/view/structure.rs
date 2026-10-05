//! The Structure, Indexes and Constraints tabs (and a view's Columns), from
//! Core's `table_metadata`: columns with their types,
//! nullability, defaults and keys; the indexes; the primary key, foreign
//! keys and unique columns. CHECK constraints aren't in the metadata, so
//! the Constraints tab says so.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::theme::Role;
use crate::state::app::Model;
use crate::state::browse::{Meta, TableMeta};
use crate::state::grid;
use crate::state::text;

/// A table of rows under a header and a rule, columns as wide as their
/// widest cell (each at most 48).
fn table(
    model: &Model,
    header: &[&str],
    rows: Vec<Vec<String>>,
    width: usize,
) -> Vec<Line<'static>> {
    let theme = &model.theme;
    // Names, types and defaults come from the database: cleaned (M5).
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|r| r.iter().map(|c| grid::clean(c)).collect())
        .collect();
    let mut widths: Vec<usize> = header.iter().map(|h| h.width()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.width()).min(48);
            }
        }
    }
    let line = |cells: Vec<String>, role: Role, bold: bool| {
        let mut style = theme.style(role);
        if bold {
            style = style.bold();
        }
        let last = cells.len().saturating_sub(1);
        let text = cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == last {
                    c.clone()
                } else {
                    grid::pad(c, widths[i], false)
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        Line::from(Span::styled(grid::fit(&text, width), style))
    };
    let mut lines = vec![
        line(
            header.iter().map(|h| h.to_string()).collect(),
            Role::Header,
            true,
        ),
        Line::from(Span::styled(
            "─".repeat(
                width.min(widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1)),
            ),
            theme.style(Role::Border),
        )),
    ];
    lines.extend(rows.into_iter().map(|r| line(r, Role::Text, false)));
    lines
}

/// The metadata, or what to say while it isn't there.
fn loaded(model: &Model) -> Result<&TableMeta, Vec<Line<'static>>> {
    let theme = &model.theme;
    match &model.browse.meta {
        Meta::Loaded(meta) => Ok(meta),
        Meta::Failed(e) => Err(vec![Line::from(Span::styled(
            text::failed_line("table metadata", &e.code),
            theme.style(Role::Deleted),
        ))]),
        Meta::Idle | Meta::Loading => Err(vec![Line::from(Span::styled(
            text::LOADING,
            theme.style(Role::Dim),
        ))]),
    }
}

/// Structure (and a view's Columns): `column  type  null  default  key`.
pub fn structure(model: &Model, width: usize) -> Vec<Line<'static>> {
    let meta = match loaded(model) {
        Ok(meta) => meta,
        Err(lines) => return lines,
    };
    let rows = meta
        .columns
        .iter()
        .map(|c| {
            let mut key = Vec::new();
            if c.is_primary_key {
                key.push("pk".to_string());
            }
            if let Some(fk) = &c.foreign_key_ref {
                key.push(format!(
                    "fk → {}.{}({})",
                    fk.referenced_schema, fk.referenced_table, fk.referenced_column
                ));
            } else if c.is_foreign_key {
                key.push("fk".to_string());
            }
            if c.is_unique {
                key.push("unique".to_string());
            }
            vec![
                c.name.clone(),
                c.ty.clone(),
                if c.nullable { "yes" } else { "no" }.to_string(),
                c.default_value.clone().unwrap_or_else(|| "—".to_string()),
                key.join(" · "),
            ]
        })
        .collect();
    table(
        model,
        &["column", "type", "null", "default", "key"],
        rows,
        width,
    )
}

/// Indexes: `name  columns  type`, with `unique` after the type.
pub fn indexes(model: &Model, width: usize) -> Vec<Line<'static>> {
    let meta = match loaded(model) {
        Ok(meta) => meta,
        Err(lines) => return lines,
    };
    if meta.indexes.is_empty() {
        return vec![Line::from(Span::styled(
            text::NO_INDEXES,
            model.theme.style(Role::Dim),
        ))];
    }
    let rows = meta
        .indexes
        .iter()
        .map(|i| {
            let mut kind = i.ty.clone();
            if i.unique {
                kind.push_str(" · unique");
            }
            vec![i.name.clone(), format!("({})", i.columns.join(", ")), kind]
        })
        .collect();
    table(model, &["name", "columns", "type"], rows, width)
}

/// Constraints: the primary key, each foreign key and each unique column
/// or unique index other than the primary key's.
pub fn constraints(model: &Model, width: usize) -> Vec<Line<'static>> {
    let meta = match loaded(model) {
        Ok(meta) => meta,
        Err(lines) => return lines,
    };
    let mut rows = Vec::new();
    let pk = meta.primary_key();
    if !pk.is_empty() {
        rows.push(vec![
            "PRIMARY KEY".to_string(),
            format!("({})", pk.join(", ")),
        ]);
    }
    for c in &meta.columns {
        if let Some(fk) = &c.foreign_key_ref {
            rows.push(vec![
                "FOREIGN KEY".to_string(),
                format!(
                    "({}) → {}.{}({})",
                    c.name, fk.referenced_schema, fk.referenced_table, fk.referenced_column
                ),
            ]);
        }
    }
    for c in meta
        .columns
        .iter()
        .filter(|c| c.is_unique && !c.is_primary_key)
    {
        rows.push(vec!["UNIQUE".to_string(), format!("({})", c.name)]);
    }
    for i in meta.indexes.iter().filter(|i| i.unique) {
        let mut cols: Vec<&str> = i.columns.iter().map(String::as_str).collect();
        let mut key = pk.clone();
        cols.sort_unstable();
        key.sort_unstable();
        if cols != key {
            rows.push(vec![
                "UNIQUE INDEX".to_string(),
                format!("{} ({})", i.name, i.columns.join(", ")),
            ]);
        }
    }
    let mut lines = if rows.is_empty() {
        vec![Line::from(Span::styled(
            text::NO_CONSTRAINTS,
            model.theme.style(Role::Dim),
        ))]
    } else {
        table(model, &["constraint", "definition"], rows, width)
    };
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        text::CHECKS_NOT_LISTED,
        model.theme.style(Role::Dim),
    )));
    lines
}

/// DDL: Core's `create_table` over the metadata, headed "approximate".
pub fn ddl(model: &Model) -> Vec<Line<'static>> {
    let theme = &model.theme;
    let meta = match loaded(model) {
        Ok(meta) => meta,
        Err(lines) => return lines,
    };
    let mut lines = vec![Line::from(Span::styled(
        text::DDL_APPROXIMATE,
        theme.style(Role::Dim),
    ))];
    match &meta.ddl {
        Ok(ddl) => lines.extend(
            ddl.lines()
                .map(|l| Line::from(Span::styled(grid::clean(l), theme.style(Role::Text)))),
        ),
        Err(e) => lines.push(Line::from(Span::styled(
            text::failed_line("DDL", &e.code),
            theme.style(Role::Deleted),
        ))),
    }
    lines
}
