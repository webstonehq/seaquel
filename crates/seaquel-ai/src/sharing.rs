//! Whether a connection shares its schema and its data with AI tools:
//! one rule for the assistant and the MCP server, moved from
//! `seaquel-mcp`'s `exposed.rs`. Core re-reads the connection row and the
//! global `aiSettings` before each tool call and passes them here.

use seaquel_types::storage::PersistedConnection;
use serde_json::Value as Json;

/// A connection's AI sharing flags after the global default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sharing {
    pub schema: bool,
    pub data: bool,
}

/// The `app_state` key the GUI keeps its AI settings under
/// (`AI_SETTINGS_KEY` in `src/lib/stores/ai-settings.svelte.ts`).
pub const AI_SETTINGS_KEY: &str = "aiSettings";

/// The GUI's rule, ported from `ui-state.svelte.ts` `_resolveAISettings`
/// (and `ai-assistant.svelte`): a connection's `aiShareSchema`/`aiShareData`
/// when set, else the global `shareSchemaGlobally`/`shareDataGlobally`.
pub fn sharing(row: &PersistedConnection, global: Sharing) -> Sharing {
    Sharing {
        schema: row.ai_share_schema.unwrap_or(global.schema),
        data: row.ai_share_data.unwrap_or(global.data),
    }
}

/// `DEFAULT_AI_SETTINGS` in `src/lib/types/ai.ts`.
pub const DEFAULT_SHARING: Sharing = Sharing {
    schema: true,
    data: false,
};

/// The stored `aiSettings` text to the global flags, exactly as
/// `initialize` then `{ ...DEFAULT_AI_SETTINGS, ...parsed, providers }` and
/// the consumers' truthiness give them:
///
/// - no value, or `""` (`if (raw)`), keeps the defaults;
/// - text `JSON.parse` rejects keeps the defaults (the `catch`);
/// - so does anything the provider migration throws on: `null` (reading
///   `parsed.providers`), a `providers` that is neither absent/`null` nor an
///   array (`.map` isn't a function), and a `null` provider entry (the
///   destructuring);
/// - otherwise a key the parsed object has replaces the default whatever its
///   value (a spread copies `null` too), and the flag is that value's
///   JavaScript truthiness. Arrays and primitives carry no such key.
pub fn global_sharing_from(raw: Option<&str>) -> Sharing {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return DEFAULT_SHARING;
    };
    let Ok(parsed) = serde_json::from_str::<Json>(raw) else {
        return DEFAULT_SHARING;
    };
    if parsed.is_null() {
        return DEFAULT_SHARING;
    }
    if let Json::Object(obj) = &parsed {
        match obj.get("providers") {
            None | Some(Json::Null) => {}
            Some(Json::Array(items)) if !items.iter().any(Json::is_null) => {}
            Some(_) => return DEFAULT_SHARING,
        }
        return Sharing {
            schema: obj
                .get("shareSchemaGlobally")
                .map_or(DEFAULT_SHARING.schema, truthy),
            data: obj
                .get("shareDataGlobally")
                .map_or(DEFAULT_SHARING.data, truthy),
        };
    }
    // `(5).providers`, `"x".providers` and `[].providers` are undefined, and
    // spreading them adds no settings key.
    DEFAULT_SHARING
}

/// JavaScript's `Boolean(v)` for a JSON value.
fn truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Json::String(s) => !s.is_empty(),
        Json::Array(_) | Json::Object(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(raw: &str) -> (bool, bool) {
        let s = global_sharing_from(Some(raw));
        (s.schema, s.data)
    }

    #[test]
    fn defaults_when_missing_empty_or_unparseable() {
        assert_eq!(global_sharing_from(None), DEFAULT_SHARING);
        assert_eq!(g(""), (true, false));
        assert_eq!(g("{not json"), (true, false));
        assert_eq!(g("null"), (true, false));
    }

    #[test]
    fn stored_flags_replace_the_defaults() {
        assert_eq!(
            g(r#"{"shareSchemaGlobally":false,"shareDataGlobally":true}"#),
            (false, true)
        );
        assert_eq!(g(r#"{"shareDataGlobally":true}"#), (true, true));
        assert_eq!(g(r#"{"enabled":true}"#), (true, false));
    }

    #[test]
    fn a_present_key_counts_by_truthiness() {
        assert_eq!(
            g(r#"{"shareSchemaGlobally":null,"shareDataGlobally":1}"#),
            (false, true)
        );
        assert_eq!(
            g(r#"{"shareSchemaGlobally":"","shareDataGlobally":"no"}"#),
            (false, true)
        );
        assert_eq!(
            g(r#"{"shareSchemaGlobally":0,"shareDataGlobally":[]}"#),
            (false, true)
        );
    }

    #[test]
    fn what_the_provider_migration_throws_on_keeps_the_defaults() {
        let off = r#""shareSchemaGlobally":false,"shareDataGlobally":true"#;
        assert_eq!(g(&format!(r#"{{"providers":5,{off}}}"#)), (true, false));
        assert_eq!(g(&format!(r#"{{"providers":{{}},{off}}}"#)), (true, false));
        assert_eq!(
            g(&format!(r#"{{"providers":[null],{off}}}"#)),
            (true, false)
        );
        assert_eq!(g(&format!(r#"{{"providers":null,{off}}}"#)), (false, true));
        assert_eq!(
            g(&format!(r#"{{"providers":[{{"id":"a"}},3],{off}}}"#)),
            (false, true)
        );
    }

    #[test]
    fn non_objects_carry_no_settings() {
        assert_eq!(g("5"), (true, false));
        assert_eq!(g("true"), (true, false));
        assert_eq!(g(r#""shareDataGlobally""#), (true, false));
        assert_eq!(g("[true, true]"), (true, false));
    }
}
