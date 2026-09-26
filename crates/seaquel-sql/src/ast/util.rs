//! Helpers over sqlparser's AST shared by the mappers (Task 6, from the phase
//! 2b spike's `ast_util.rs`). They turn sqlparser nodes into the pieces
//! node-sql-parser handed the TS code directly: `column_ref`, `aggr_func`, a
//! flat FROM list.

use sqlparser::ast::{
    Expr, FunctionArg, FunctionArgExpr, FunctionArguments, JoinConstraint, JoinOperator,
    ObjectName, Query, Select, SetExpr, Statement, TableFactor, TableWithJoins, UnaryOperator,
    Value,
};
use sqlparser::keywords::Keyword;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer};

use super::dialect::for_engine;
use crate::SqlEngine;

/// How deep an expression may nest before parsing refuses it. See
/// [`chain_depth`]. A 1,000-term `id = 1 OR id = 2 …` is exactly this. On a
/// 1 MB native release stack the first overflow (with no cap) is a WHERE or
/// HAVING chain at about 10,000 by this count, a 5× margin; the module links
/// a 2 MB stack. `tests/ast_nesting.rs` has the other measurements.
pub(crate) const MAX_CHAIN_DEPTH: usize = 2000;

/// The error for input past [`MAX_CHAIN_DEPTH`] (sqlparser adds its
/// `sql parser error: ` prefix, as for its own recursion limit).
pub(crate) const TOO_DEEP: &str = "query is nested too deeply to parse";

/// Parse `sql` in the engine's dialect, as `Parser::parse_sql` does.
///
/// sqlparser's recursion limit (50) stops deep parentheses and subqueries, but
/// not operator chains: `a AND b AND c …`, `a + b + …`, `x::int::int …` and
/// `SELECT … UNION SELECT … UNION …` are built in a loop, as left-deep trees
/// of any depth. Dropping one recurses once per level: 20,000 `AND`s overflow
/// a 1 MB stack inside sqlparser itself, before any mapper here walks it. So the tokens are checked first, and input whose chains could
/// nest past [`MAX_CHAIN_DEPTH`] is a parse error. Then every AST this crate
/// sees is at most that deep, which bounds the mappers' recursion too.
pub(crate) fn parse(sql: &str, engine: SqlEngine) -> Result<Vec<Statement>, ParserError> {
    parse_noting_not_eq(sql, engine).map(|p| p.statements)
}

/// A parse, and where each `!=` or `<>` token starts, in order.
pub(crate) struct Parsed {
    pub statements: Vec<Statement>,
    /// Each `!=`/`<>` token's start, and whether it was written `!=`.
    /// sqlparser reads both as `NotEq`; the Visual tab prints what was written.
    pub not_eq: Vec<(Location, bool)>,
}

/// [`parse`], noting how each `<>`/`!=` was written.
pub(crate) fn parse_noting_not_eq(sql: &str, engine: SqlEngine) -> Result<Parsed, ParserError> {
    let dialect = for_engine(engine);
    // `Parser::try_with_sql`, with the tokens kept for the check.
    let tokens = Tokenizer::new(dialect.as_ref(), sql)
        .with_unescape(true)
        .tokenize_with_location()?;
    if chain_depth(&tokens) > MAX_CHAIN_DEPTH {
        return Err(ParserError::ParserError(TOO_DEEP.to_string()));
    }
    let lines = LineIndex::new(sql);
    let not_eq = tokens
        .iter()
        .filter(|t| t.token == Token::Neq)
        .map(|t| {
            let loc = t.span.start;
            let bang = lines
                .offset(loc)
                .and_then(|i| sql.get(i..))
                .is_some_and(|rest| rest.starts_with('!'));
            (loc, bang)
        })
        .collect();
    let statements = Parser::new(dialect.as_ref())
        .with_tokens_with_locations(tokens)
        .parse_statements()?;
    Ok(Parsed { statements, not_eq })
}

/// Turns sqlparser locations (1-based line, 1-based column counting chars;
/// lines end at `\n`) into byte offsets.
struct LineIndex<'a> {
    src: &'a str,
    /// Byte offset of each line's start.
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(src: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { src, starts }
    }

    /// The byte offset, or `None` for an empty span or one outside the text.
    fn offset(&self, loc: Location) -> Option<usize> {
        let line = usize::try_from(loc.line).ok()?.checked_sub(1)?;
        let col = usize::try_from(loc.column).ok()?.checked_sub(1)?;
        let start = *self.starts.get(line)?;
        let rest = self.src.get(start..)?;
        rest.char_indices().nth(col).map(|(i, _)| start + i)
    }
}

/// An upper bound on how deep the parsed tree's operator chains nest.
///
/// A chain never spans a `,` (select items, arguments, list items and rows are
/// separate subtrees) or a `;`, and each parenthesised group adds its own. So
/// a group weighs the most, over its comma-separated segments, of the
/// operators in the segment plus the heaviest group inside it. Every token
/// that could be an infix or postfix operator counts, so this over-counts
/// (`SELECT *` counts one), never under-counts.
pub(crate) fn chain_depth(tokens: &[TokenWithSpan]) -> usize {
    #[derive(Default)]
    struct Group {
        /// Operators in the current segment.
        ops: usize,
        /// The heaviest group closed inside the current segment.
        child: usize,
        /// The heaviest segment so far.
        best: usize,
    }
    impl Group {
        fn end_segment(&mut self) {
            self.best = self.best.max(self.ops + self.child);
            self.ops = 0;
            self.child = 0;
        }
        fn weight(mut self) -> usize {
            self.end_segment();
            self.best
        }
    }

    let mut max = 0;
    let mut stack: Vec<Group> = vec![Group::default()];
    for t in tokens {
        match &t.token {
            Token::LParen => stack.push(Group::default()),
            Token::RParen => {
                if stack.len() > 1 {
                    if let Some(g) = stack.pop() {
                        let w = g.weight();
                        if let Some(parent) = stack.last_mut() {
                            parent.child = parent.child.max(w);
                        }
                    }
                }
            }
            Token::Comma => {
                if let Some(g) = stack.last_mut() {
                    g.end_segment();
                }
            }
            Token::SemiColon => {
                // A new statement: whatever is still open ends here.
                while let Some(g) = stack.pop() {
                    let w = g.weight();
                    match stack.last_mut() {
                        Some(parent) => parent.child = parent.child.max(w),
                        None => max = max.max(w),
                    }
                }
                stack.push(Group::default());
            }
            tok if counts_as_operator(tok) => {
                if let Some(g) = stack.last_mut() {
                    g.ops += 1;
                }
            }
            _ => {}
        }
    }
    while let Some(g) = stack.pop() {
        let w = g.weight();
        match stack.last_mut() {
            Some(parent) => parent.child = parent.child.max(w),
            None => max = max.max(w),
        }
    }
    max
}

/// Whether a token could start an infix or postfix step of a chain: every
/// symbol but `,` `(` `)` `;` `.`, and the keywords that combine expressions
/// or queries.
fn counts_as_operator(t: &Token) -> bool {
    match t {
        Token::Word(w) => matches!(
            w.keyword,
            Keyword::AND
                | Keyword::OR
                | Keyword::XOR
                | Keyword::NOT
                | Keyword::IS
                | Keyword::IN
                | Keyword::BETWEEN
                | Keyword::LIKE
                | Keyword::ILIKE
                | Keyword::SIMILAR
                | Keyword::REGEXP
                | Keyword::RLIKE
                | Keyword::GLOB
                | Keyword::MATCH
                | Keyword::COLLATE
                | Keyword::AT
                | Keyword::DIV
                | Keyword::MOD
                | Keyword::OVERLAPS
                | Keyword::MEMBER
                | Keyword::OPERATOR
                | Keyword::UNION
                | Keyword::INTERSECT
                | Keyword::EXCEPT
                | Keyword::MINUS
        ),
        Token::EOF
        | Token::Whitespace(_)
        | Token::Comma
        | Token::LParen
        | Token::RParen
        | Token::SemiColon
        | Token::Period
        | Token::Number(..)
        | Token::Char(_)
        | Token::SingleQuotedString(_)
        | Token::DoubleQuotedString(_)
        | Token::TripleSingleQuotedString(_)
        | Token::TripleDoubleQuotedString(_)
        | Token::DollarQuotedString(_)
        | Token::SingleQuotedByteStringLiteral(_)
        | Token::DoubleQuotedByteStringLiteral(_)
        | Token::TripleSingleQuotedByteStringLiteral(_)
        | Token::TripleDoubleQuotedByteStringLiteral(_)
        | Token::SingleQuotedRawStringLiteral(_)
        | Token::DoubleQuotedRawStringLiteral(_)
        | Token::TripleSingleQuotedRawStringLiteral(_)
        | Token::TripleDoubleQuotedRawStringLiteral(_)
        | Token::NationalStringLiteral(_)
        | Token::QuoteDelimitedStringLiteral(_)
        | Token::NationalQuoteDelimitedStringLiteral(_)
        | Token::EscapedStringLiteral(_)
        | Token::UnicodeStringLiteral(_)
        | Token::HexStringLiteral(_)
        | Token::Placeholder(_) => false,
        _ => true,
    }
}

/// `column_ref`: optional table qualifier plus column name.
pub(crate) fn column_ref(e: &Expr) -> Option<(Option<String>, String)> {
    match e {
        Expr::Identifier(i) => Some((None, i.value.clone())),
        Expr::CompoundIdentifier(parts) if parts.len() >= 2 => Some((
            Some(parts[parts.len() - 2].value.clone()),
            parts[parts.len() - 1].value.clone(),
        )),
        _ => None,
    }
}

/// Last part of an object name, and the one before it (the schema).
pub(crate) fn table_and_schema(name: &ObjectName) -> (String, Option<String>) {
    let parts: Vec<&str> = name
        .0
        .iter()
        .filter_map(|p| p.as_ident().map(|i| i.value.as_str()))
        .collect();
    let table = parts.last().copied().unwrap_or_default().to_string();
    let schema = if parts.len() >= 2 {
        Some(parts[parts.len() - 2].to_string())
    } else {
        None
    };
    (table, schema)
}

/// The leftmost plain SELECT of a query body (node-sql-parser types a UNION
/// as its first SELECT with a `_next` link).
pub(crate) fn first_select(q: &Query) -> Option<&Select> {
    // A loop, not recursion: `((((SELECT 1))))` nests `SetExpr::Query`.
    let mut s = q.body.as_ref();
    loop {
        match s {
            SetExpr::Select(sel) => return Some(sel),
            SetExpr::Query(q) => s = q.body.as_ref(),
            SetExpr::SetOperation { left, .. } => s = left.as_ref(),
            _ => return None,
        }
    }
}

/// The join kinds node-sql-parser writes in a FROM entry's `join`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FromJoin {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    /// `CROSS APPLY`, `OUTER APPLY`, `SEMI JOIN`, `ASOF JOIN`, …: none of the
    /// words the TS looks for, so both mappers read it as INNER.
    Other,
}

/// One entry of node-sql-parser's flat FROM list.
pub(crate) struct FromItem<'a> {
    pub factor: &'a TableFactor,
    /// `None` for the first relation of each comma-separated item.
    pub join: Option<FromJoin>,
    pub on: Option<&'a Expr>,
}

pub(crate) fn flat_from(from: &[TableWithJoins]) -> Vec<FromItem<'_>> {
    let mut out = Vec::new();
    for twj in from {
        out.push(FromItem {
            factor: &twj.relation,
            join: None,
            on: None,
        });
        for j in &twj.joins {
            let (kind, constraint) = match &j.join_operator {
                JoinOperator::Join(c) | JoinOperator::Inner(c) => (FromJoin::Inner, Some(c)),
                JoinOperator::Left(c) | JoinOperator::LeftOuter(c) => (FromJoin::Left, Some(c)),
                JoinOperator::Right(c) | JoinOperator::RightOuter(c) => (FromJoin::Right, Some(c)),
                JoinOperator::FullOuter(c) => (FromJoin::Full, Some(c)),
                JoinOperator::CrossJoin(c) => (FromJoin::Cross, Some(c)),
                JoinOperator::CrossApply => (FromJoin::Cross, None),
                _ => (FromJoin::Other, None),
            };
            let on = match constraint {
                Some(JoinConstraint::On(e)) => Some(e),
                _ => None,
            };
            out.push(FromItem {
                factor: &j.relation,
                join: Some(kind),
                on,
            });
        }
    }
    out
}

/// Strip `( ... )` around an expression. node-sql-parser keeps the node and
/// sets `parentheses: true`, so the TS code never sees the wrapper.
pub(crate) fn unnest(mut e: &Expr) -> &Expr {
    while let Expr::Nested(inner) = e {
        e = inner;
    }
    e
}

/// What an aggregate's single argument is, as the TS code distinguishes it.
pub(crate) enum AggArg {
    Star,
    Column(Option<String>, String),
    Other,
}

/// A function call's name, upper-cased, and what its first argument is: the
/// pieces of an `aggr_func` node.
pub(crate) fn aggregate(e: &Expr) -> Option<(String, Option<AggArg>)> {
    let Expr::Function(f) = e else { return None };
    let name = f.name.0.last()?.as_ident()?.value.to_uppercase();
    let arg = match &f.args {
        FunctionArguments::List(list) => list.args.first().map(|a| match a {
            FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => AggArg::Star,
            FunctionArg::Unnamed(FunctionArgExpr::Expr(x)) => match column_ref(x) {
                Some((t, c)) => AggArg::Column(t, c),
                None => AggArg::Other,
            },
            _ => AggArg::Other,
        }),
        _ => None,
    };
    Some((name, arg))
}

/// node-sql-parser literal → the string `extractValue` returns.
pub(crate) fn literal(e: &Expr) -> Option<String> {
    match e {
        Expr::Value(v) => match &v.value {
            // node-sql-parser keeps a decimal's text ("50.0" stays "50.0").
            Value::Number(n, _) => Some(n.clone()),
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => Some(s.clone()),
            _ => None,
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match expr.as_ref() {
            // One level only: `- -1` is no literal to the TS either.
            Expr::Value(_) => literal(expr).map(|v| format!("-{v}")),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depth(sql: &str) -> usize {
        let d = for_engine(SqlEngine::Postgres);
        let tokens = Tokenizer::new(d.as_ref(), sql)
            .tokenize_with_location()
            .expect("tokenizes");
        chain_depth(&tokens)
    }

    #[test]
    fn chain_depth_counts_operators_per_segment() {
        assert_eq!(depth(""), 0);
        assert_eq!(depth("SELECT a FROM t"), 0);
        // `*` counts: an over-count is safe.
        assert_eq!(depth("SELECT * FROM t WHERE a = 1 AND b = 2"), 4);
        // Commas and statements separate chains.
        assert_eq!(depth("SELECT a + 1, b + 1, c + 1 + 1 FROM t"), 2);
        assert_eq!(depth("SELECT 1 + 1; SELECT 1 + 1 + 1"), 2);
        // A group adds its heaviest segment to the segment it sits in.
        assert_eq!(depth("SELECT (a + (b + c), d + e + f + g) + 1"), 4);
        // Keywords that chain, and set operations.
        assert_eq!(depth("SELECT 1 UNION SELECT 2 UNION ALL SELECT 3"), 2);
        assert_eq!(depth("SELECT a IS NOT NULL"), 2);
        assert_eq!(depth("SELECT a::int::text"), 2);
        // Strings, numbers and placeholders don't count, whatever they hold.
        assert_eq!(depth("SELECT 'a AND b', $1, 1.5e3"), 0);
        // Unbalanced input still ends with a number.
        assert_eq!(depth("SELECT ((a + b"), 1);
        assert_eq!(depth("SELECT a + b))) + c"), 2);
    }

    #[test]
    fn deep_input_is_a_parse_error_before_it_is_built() {
        let sql = format!("SELECT {}", vec!["1"; MAX_CHAIN_DEPTH + 2].join(" + "));
        let err = parse(&sql, SqlEngine::Postgres).expect_err("too deep");
        assert_eq!(err.to_string(), format!("sql parser error: {TOO_DEEP}"));
        let sql = format!("SELECT {}", vec!["1"; MAX_CHAIN_DEPTH + 1].join(" + "));
        assert!(parse(&sql, SqlEngine::Postgres).is_ok());
    }
}
