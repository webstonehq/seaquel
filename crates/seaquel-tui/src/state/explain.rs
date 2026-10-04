//! The Explain tab (Decision 14; design 1b): Core's plan as a tree with
//! each node's rows, its own time and its share of the total. A node's own
//! ("self") time is `actual_total_time × actual_loops` less its children's
//! (Postgres reports times per loop); the share is that over the sum of
//! every node's own time, which is the root's inclusive time. A plain
//! EXPLAIN has no times: the tree shows the planner's rows and cost.

use std::fmt;

use seaquel_types::{ExplainPlanNode, ExplainResult};

use super::grid;

/// One row of the tree.
#[derive(Clone, PartialEq)]
pub struct PlanRow {
    /// The tree's drawing before the label (`├─ `, `│  └─ `, …).
    pub prefix: String,
    /// The node and what it works on: `Seq Scan invoice_line_items li`.
    pub label: String,
    /// Rows out: actual (× loops) under ANALYZE, else the estimate.
    pub rows: Option<f64>,
    /// Own time in ms (ANALYZE only).
    pub self_ms: Option<f64>,
    /// Own time as a share of the total, 0–100 (ANALYZE only).
    pub share: Option<f64>,
    /// The planner's total cost (plain EXPLAIN).
    pub cost: Option<f64>,
}

impl fmt::Debug for PlanRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlanRow")
            .field("rows", &self.rows)
            .field("self_ms", &self.self_ms)
            .field("share", &self.share)
            .finish_non_exhaustive()
    }
}

/// A node's inclusive time over all its loops.
pub fn inclusive_ms(node: &ExplainPlanNode) -> Option<f64> {
    let total = node.actual_total_time?;
    Some(total * node.actual_loops.unwrap_or(1).max(1) as f64)
}

/// A node's own time: its inclusive time less its children's, never below
/// zero (rounding in the server's numbers).
pub fn self_ms(node: &ExplainPlanNode) -> Option<f64> {
    let own = inclusive_ms(node)?;
    let children: f64 = node.children.iter().filter_map(inclusive_ms).sum();
    Some((own - children).max(0.0))
}

/// The node's label: its type and the relation, index, join or sort it
/// names.
pub fn label(node: &ExplainPlanNode) -> String {
    let unwrap = |c: &str| {
        let c = c.trim();
        c.strip_prefix('(')
            .and_then(|c| c.strip_suffix(')'))
            .unwrap_or(c)
            .to_string()
    };
    let detail = if let Some(cond) = &node.hash_cond {
        Some(unwrap(cond))
    } else if let Some(index) = &node.index_name {
        Some(index.clone())
    } else if let Some(relation) = &node.relation_name {
        Some(match &node.alias {
            Some(alias) if alias != relation => format!("{relation} {alias}"),
            _ => relation.clone(),
        })
    } else if let Some(keys) = node.sort_key.as_ref().filter(|k| !k.is_empty()) {
        Some(keys.join(", "))
    } else {
        node.index_cond
            .as_deref()
            .or(node.filter.as_deref())
            .map(unwrap)
    };
    let text = match detail {
        Some(detail) if !detail.is_empty() => format!("{} {detail}", node.node_type),
        _ => node.node_type.clone(),
    };
    grid::clean(&text)
}

/// The tree, depth first, as the tab draws it.
pub fn rows(result: &ExplainResult) -> Vec<PlanRow> {
    fn own_total(node: &ExplainPlanNode) -> f64 {
        self_ms(node).unwrap_or(0.0) + node.children.iter().map(own_total).sum::<f64>()
    }
    let total = own_total(&result.plan);
    let mut out = Vec::new();
    // (node, the drawing of its ancestors' lines, is it the last child,
    // is it the root)
    let mut stack: Vec<(&ExplainPlanNode, String, bool, bool)> =
        vec![(&result.plan, String::new(), true, true)];
    while let Some((node, lead, last, root)) = stack.pop() {
        let prefix = if root {
            String::new()
        } else {
            format!("{lead}{}", if last { "└─ " } else { "├─ " })
        };
        let own = self_ms(node);
        let rows = match (node.actual_rows, result.is_analyze) {
            (Some(rows), true) => Some(rows * node.actual_loops.unwrap_or(1).max(1) as f64),
            _ => node.plan_rows,
        };
        out.push(PlanRow {
            prefix,
            label: label(node),
            rows,
            self_ms: own,
            share: own.map(|o| if total > 0.0 { o / total * 100.0 } else { 0.0 }),
            cost: node.total_cost,
        });
        let child_lead = if root {
            String::new()
        } else {
            format!("{lead}{}", if last { "   " } else { "│  " })
        };
        let n = node.children.len();
        for (i, child) in node.children.iter().enumerate().rev() {
            stack.push((child, child_lead.clone(), i + 1 == n, false));
        }
    }
    out
}

/// The tab's title on the right: `ANALYZE ✓ · 412.3 ms`, or `EXPLAIN ·
/// cost 1234.5` for a plain plan.
pub fn header(result: &ExplainResult) -> String {
    match (result.is_analyze, result.execution_time) {
        (true, Some(ms)) => format!("ANALYZE ✓ · {ms:.1} ms"),
        (true, None) => "ANALYZE ✓".to_string(),
        (false, _) => match result.plan.total_cost {
            Some(cost) => format!("EXPLAIN · cost {cost:.1}"),
            None => "EXPLAIN".to_string(),
        },
    }
}

/// `1,204` for a row count (a fractional per-loop average rounds).
pub fn rows_text(rows: f64) -> String {
    grid::thousands(rows.max(0.0).round() as u64)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn node(
        node_type: &str,
        total: f64,
        loops: i64,
        rows: f64,
        children: Vec<ExplainPlanNode>,
    ) -> ExplainPlanNode {
        ExplainPlanNode {
            id: node_type.to_string(),
            node_type: node_type.to_string(),
            relation_name: None,
            alias: None,
            startup_cost: Some(0.0),
            total_cost: Some(100.0),
            plan_rows: Some(rows),
            plan_width: Some(8),
            actual_startup_time: Some(0.0),
            actual_total_time: Some(total),
            actual_rows: Some(rows),
            actual_loops: Some(loops),
            filter: None,
            index_name: None,
            index_cond: None,
            join_type: None,
            hash_cond: None,
            sort_key: None,
            children,
        }
    }

    /// The design's 1b plan (its numbers).
    pub(crate) fn design_plan() -> ExplainResult {
        let scan = |relation: &str, alias: &str, total: f64, rows: f64| ExplainPlanNode {
            relation_name: Some(relation.into()),
            alias: Some(alias.into()),
            ..node("Seq Scan", total, 1, rows, vec![])
        };
        let index = ExplainPlanNode {
            index_name: Some("invoices_issued_at_idx".into()),
            relation_name: Some("invoices".into()),
            ..node("Index Scan", 33.6, 1, 31_877.0, vec![])
        };
        let inner_join = ExplainPlanNode {
            hash_cond: Some("(c.id = i.customer_id)".into()),
            join_type: Some("Inner".into()),
            ..node(
                "Hash Join",
                58.3,
                1,
                31_877.0,
                vec![index, scan("customers", "c", 5.3, 12_418.0)],
            )
        };
        let hash = node("Hash", 64.5, 1, 31_877.0, vec![inner_join]);
        let outer_join = ExplainPlanNode {
            hash_cond: Some("(li.invoice_id = i.id)".into()),
            join_type: Some("Inner".into()),
            ..node(
                "Hash Join",
                364.4,
                1,
                298_410.0,
                vec![scan("invoice_line_items", "li", 238.9, 312_006.0), hash],
            )
        };
        let aggregate = node("HashAggregate", 409.2, 1, 1_204.0, vec![outer_join]);
        let sort = ExplainPlanNode {
            sort_key: Some(vec!["(sum(revenue)) DESC".into()]),
            ..node("Sort", 412.3, 1, 1_204.0, vec![aggregate])
        };
        ExplainResult {
            plan: sort,
            planning_time: 0.4,
            execution_time: Some(412.3),
            is_analyze: true,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn self_time_is_inclusive_less_children_times_loops() {
        let plan = design_plan();
        assert!(close(inclusive_ms(&plan.plan).unwrap(), 412.3));
        assert!(close(self_ms(&plan.plan).unwrap(), 412.3 - 409.2));
        // Loops multiply a per-loop time.
        let inner = node("Index Scan", 0.5, 10, 2.0, vec![]);
        let outer = node("Nested Loop", 7.0, 1, 20.0, vec![inner]);
        assert!(close(inclusive_ms(&outer.children[0]).unwrap(), 5.0));
        assert!(close(self_ms(&outer).unwrap(), 2.0));
        // Never below zero.
        let odd = node(
            "Gather",
            1.0,
            1,
            1.0,
            vec![node("Seq Scan", 2.0, 1, 1.0, vec![])],
        );
        assert_eq!(self_ms(&odd), Some(0.0));
        // No times without ANALYZE.
        let mut plain = node("Seq Scan", 0.0, 1, 1.0, vec![]);
        plain.actual_total_time = None;
        plain.actual_loops = None;
        assert_eq!(self_ms(&plain), None);
    }

    // Design 1b: the tree, its labels and the shares that add up to the
    // total.
    #[test]
    fn the_design_s_tree() {
        let rows = rows(&design_plan());
        let drawn: Vec<String> = rows
            .iter()
            .map(|r| format!("{}{}", r.prefix, r.label))
            .collect();
        assert_eq!(
            drawn,
            [
                "Sort (sum(revenue)) DESC",
                "└─ HashAggregate",
                "   └─ Hash Join li.invoice_id = i.id",
                "      ├─ Seq Scan invoice_line_items li",
                "      └─ Hash",
                "         └─ Hash Join c.id = i.customer_id",
                "            ├─ Index Scan invoices_issued_at_idx",
                "            └─ Seq Scan customers c",
            ]
        );
        let shares: f64 = rows.iter().map(|r| r.share.unwrap()).sum();
        assert!(close(shares, 100.0), "{shares}");
        let seq = &rows[3];
        assert!(close(seq.self_ms.unwrap(), 238.9));
        assert_eq!(seq.share.unwrap().round(), 58.0);
        assert_eq!(rows_text(seq.rows.unwrap()), "312,006");
        assert_eq!(header(&design_plan()), "ANALYZE ✓ · 412.3 ms");
    }

    #[test]
    fn a_plain_plan_shows_estimates_and_cost() {
        let mut plan = design_plan();
        plan.is_analyze = false;
        plan.execution_time = None;
        fn strip(n: &mut ExplainPlanNode) {
            n.actual_total_time = None;
            n.actual_loops = None;
            n.actual_rows = None;
            n.children.iter_mut().for_each(strip);
        }
        strip(&mut plan.plan);
        let rows = rows(&plan);
        assert!(rows
            .iter()
            .all(|r| r.self_ms.is_none() && r.share.is_none()));
        assert_eq!(rows[0].cost, Some(100.0));
        assert_eq!(rows[0].rows, Some(1_204.0), "the estimate");
        assert_eq!(header(&plan), "EXPLAIN · cost 100.0");
    }

    /// The Postgres EXPLAIN ANALYZE Core answered for a join over
    /// `generate_series`, recorded from the compose container
    /// (`runtime::query_tests::record_the_postgres_explain_fixture`).
    pub(crate) fn recorded() -> ExplainResult {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/explain/postgres_analyze.json");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn the_recorded_postgres_plan() {
        let plan = recorded();
        assert!(plan.is_analyze);
        let rows = rows(&plan);
        fn count(n: &ExplainPlanNode) -> usize {
            1 + n.children.iter().map(count).sum::<usize>()
        }
        assert_eq!(rows.len(), count(&plan.plan));
        let shares: f64 = rows.iter().map(|r| r.share.unwrap()).sum();
        assert!((shares - 100.0).abs() < 1e-6, "{shares}");
        let own: f64 = rows.iter().map(|r| r.self_ms.unwrap()).sum();
        assert!(
            (own - inclusive_ms(&plan.plan).unwrap()).abs() < 1e-6,
            "every node's own time adds up to the root's"
        );
        assert!(rows[0].prefix.is_empty());
        assert!(rows[1..].iter().all(|r| r.prefix.ends_with("─ ")));
        assert!(header(&plan).starts_with("ANALYZE ✓ · "));
        // The recorded labels name the nodes Postgres chose.
        assert!(
            rows.iter().any(|r| r.label.starts_with("Function Scan")),
            "{:?}",
            rows.iter().map(|r| &r.label).collect::<Vec<_>>()
        );
    }
}
