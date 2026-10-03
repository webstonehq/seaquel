//! The tools as `tools/list` gives them equal the frozen MCP profile in
//! `seaquel-ai`'s `tests/fixtures/tool-schemas.json` (Decision 20), byte for
//! byte. `seaquel-ai`'s registry is checked against the same file, so the
//! registry's MCP profile is the schemas MCP hosts already see.

use serde_json::Value as Json;

const SCHEMAS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-ai/tests/fixtures/tool-schemas.json"
);

#[test]
fn the_listed_tools_equal_the_frozen_mcp_profile() {
    let frozen: Json = serde_json::from_str(&std::fs::read_to_string(SCHEMAS).unwrap()).unwrap();
    let listed = serde_json::to_value(seaquel_mcp::McpServer::tool_list()).unwrap();
    assert_eq!(listed.to_string(), frozen["mcp"].to_string());
}
