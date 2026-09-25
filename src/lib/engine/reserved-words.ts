/**
 * Words each engine won't take as a bare part of `schema.table`, for
 * `editorQualifiedTable`. Generated once from the engines' own catalogs or
 * docs (sources per list) and checked live: every word that made
 * `SELECT 1 FROM w.t` or `SELECT 1 FROM s.w` a syntax error, out of the
 * union of all engines' keyword lists (~1,150 words), is in its engine's
 * list.
 */

import type { DatabaseType } from "$lib/types";

/** 101 words. Postgres 16 `pg_get_keywords()`, categories R (reserved) and T (type_func_name). */
const POSTGRES = `
  all analyse analyze and any array as asc asymmetric authorization binary both case cast
  check collate collation column concurrently constraint create cross current_catalog
  current_date current_role current_schema current_time current_timestamp current_user default
  deferrable desc distinct do else end except false fetch for foreign freeze from full grant
  group having ilike in initially inner intersect into is isnull join lateral leading left
  like limit localtime localtimestamp natural not notnull null offset on only or order outer
  overlaps placing primary references returning right select session_user similar some
  symmetric system_user table tablesample then to trailing true union unique user using
  variadic verbose when where window with
`;

/** 259 words. MySQL 8 `information_schema.KEYWORDS WHERE RESERVED = 1` (MySQL and MariaDB accept these in a two-part name, checked live, so this errs on the side of quoting). */
const MYSQL = `
  accessible add all alter analyze and as asc asensitive before between bigint binary blob
  both by call cascade case change char character check collate column condition constraint
  continue convert create cross cume_dist current_date current_time current_timestamp
  current_user cursor database databases day_hour day_microsecond day_minute day_second dec
  decimal declare default delayed delete dense_rank desc describe deterministic distinct
  distinctrow div double drop dual each else elseif empty enclosed escaped except exists exit
  explain false fetch first_value float float4 float8 for force foreign from fulltext function
  generated get grant group grouping groups having high_priority hour_microsecond hour_minute
  hour_second if ignore in index infile inner inout insensitive insert int int1 int2 int3 int4
  int8 integer intersect interval into io_after_gtids io_before_gtids is iterate join
  json_table key keys kill lag last_value lateral lead leading leave left like limit linear
  lines load localtime localtimestamp lock long longblob longtext loop low_priority match
  maxvalue mediumblob mediumint mediumtext middleint minute_microsecond minute_second mod
  modifies natural no_write_to_binlog not nth_value ntile null numeric of on optimize
  optimizer_costs option optionally or order out outer outfile over partition percent_rank
  precision primary procedure purge range rank read read_write reads real recursive references
  regexp release rename repeat replace require resignal restrict return revoke right rlike row
  row_number rows schema schemas second_microsecond select sensitive separator set show signal
  smallint spatial specific sql sql_big_result sql_calc_found_rows sql_small_result
  sqlexception sqlstate sqlwarning ssl starting stored straight_join system table terminated
  then tinyblob tinyint tinytext to trailing trigger true undo union unique unlock unsigned
  update usage use using utc_date utc_time utc_timestamp values varbinary varchar varcharacter
  varying virtual when where while window with write xor year_month zerofill
`;

/** 147 words. SQLite's keyword list (sqlite.org/lang_keywords.html). */
const SQLITE = `
  abort action add after all alter always analyze and as asc attach autoincrement before begin
  between by cascade case cast check collate column commit conflict constraint create cross
  current current_date current_time current_timestamp database default deferrable deferred
  delete desc detach distinct do drop each else end escape except exclude exclusive exists
  explain fail filter first following for foreign from full generated glob group groups having
  if ignore immediate in index indexed initially inner insert instead intersect into is isnull
  join key last left like limit match materialized natural no not nothing notnull null nulls
  of offset on or order others outer over partition plan pragma preceding primary query raise
  range recursive references regexp reindex release rename replace restrict returning right
  rollback row rows savepoint select set table temp temporary then ties to transaction trigger
  unbounded union unique update using vacuum values view virtual when where window with
  without
`;

/** 312 words. T-SQL reserved keywords plus ODBC reserved keywords (learn.microsoft.com, Reserved Keywords). */
const MSSQL = `
  absolute action ada add all allocate alter and any are as asc assertion at authorization avg
  backup begin between bit bit_length both break browse bulk by cascade cascaded case cast
  catalog char char_length character character_length check checkpoint close clustered
  coalesce collate collation column commit compute connect connection constraint constraints
  contains containstable continue convert corresponding count create cross current
  current_date current_time current_timestamp current_user cursor database date day dbcc
  deallocate dec decimal declare default deferrable deferred delete deny desc describe
  descriptor diagnostics disconnect disk distinct distributed domain double drop dump else end
  end-exec errlvl escape except exception exec execute exists exit external extract false
  fetch file fillfactor first float for foreign fortran found freetext freetexttable from full
  function get global go goto grant group having holdlock hour identity identity_insert
  identitycol if immediate in include index indicator initially inner input insensitive insert
  int integer intersect interval into is isolation join key kill language last leading left
  level like lineno load local lower match max merge min minute module month names national
  natural nchar next no nocheck nonclustered none not null nullif numeric octet_length of off
  offsets on only open opendatasource openquery openrowset openxml option or order outer
  output over overlaps pad partial pascal percent pivot plan position precision prepare
  preserve primary print prior privileges proc procedure public raiserror read readtext real
  reconfigure references relative replication restore restrict return revert revoke right
  rollback rowcount rowguidcol rows rule save schema scroll second section securityaudit
  select semantickeyphrasetable semanticsimilaritydetailstable semanticsimilaritytable session
  session_user set setuser shutdown size smallint some space sql sqlca sqlcode sqlerror
  sqlstate sqlwarning statistics substring sum system_user table tablesample temporary
  textsize then time timestamp timezone_hour timezone_minute to top trailing tran transaction
  translate translation trigger trim true truncate try_convert tsequal union unique unknown
  unpivot update updatetext upper usage use user using value values varchar varying view
  waitfor when whenever where while with within work write writetext year zone
`;

/** 110 words. DuckDB 1.4 `duckdb_keywords()`, categories reserved and type_function. */
const DUCKDB = `
  all analyse analyze and anti any array as asc asof asymmetric at authorization binary both
  by case cast check collate collation column columns concurrently constraint create cross
  default deferrable desc describe distinct do else end except false fetch for foreign freeze
  from full generated glob group having ilike in initially inner intersect into is isnull join
  lambda lateral leading left like limit map natural not notnull null offset on only or order
  outer overlaps pivot pivot_longer pivot_wider placing positional primary qualify references
  returning right select semi show similar some struct summarize symmetric table tablesample
  then to trailing true try_cast union unique unpack unpivot using variadic verbose when where
  window with
`;

const words = (list: string) => new Set(list.trim().split(/\s+/));

export const RESERVED_WORDS: Record<DatabaseType, ReadonlySet<string>> = {
  postgres: words(POSTGRES),
  mysql: words(MYSQL),
  mariadb: words(MYSQL),
  sqlite: words(SQLITE),
  mssql: words(MSSQL),
  duckdb: words(DUCKDB),
};
