//! sqlparser-rs, used only where an AST is needed (Task 6): the query builder
//! and tutorial `ParsedQuery`, the Visual tab's AST and column sources. Moved
//! from the phase 2b spike's `sql-spike` crate (deleted; the report is
//! `docs/plans/2026-09-27-phase-2b-spike.md`).
//!
//! Every entry point parses through `util::parse`, which refuses input whose
//! operator chains could nest deeper than sqlparser's own recursion limit
//! allows for, so nothing here can run out of stack on user input.

mod column_refs;
mod dialect;
mod tutorial;
mod util;
mod visual;

pub use column_refs::{column_refs, ColumnRef};
pub use tutorial::{
    parse_builder_query, AggregateFunction, Connector, FilterOperator, HavingOperator, JoinType,
    ParsedColumnAggregate, ParsedCte, ParsedFilter, ParsedGroupBy, ParsedHaving, ParsedJoin,
    ParsedOrderBy, ParsedQuery, ParsedSelectAggregate, ParsedSubquery, ParsedTable, SortDirection,
    SubqueryRole, TutorialSchema,
};
pub use visual::{
    parse_error, parse_visual, Limit, ParsedQueryVisual, QueryFilter, QueryJoin, QueryJoinType,
    QueryOrderBy, QueryProjection, QuerySource, QuerySourceType, VisualQueryType,
};
