use log::kv::{Key, Value, VisitSource};
use log::Record;
use std::fmt::Arguments;
use tauri_plugin_log::fern::FormatCallback;
use time::macros::format_description;

fn timestamp_now(strategy: &tauri_plugin_log::TimezoneStrategy) -> String {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
    strategy
        .get_now()
        .format(&format)
        .unwrap_or_else(|_| String::from("?"))
}

struct KvCollector {
    pairs: Vec<(String, String)>,
}

impl KvCollector {
    fn new() -> Self {
        Self { pairs: Vec::new() }
    }
}

impl<'kvs> VisitSource<'kvs> for KvCollector {
    fn visit_pair(&mut self, key: Key<'kvs>, value: Value<'kvs>) -> Result<(), log::kv::Error> {
        self.pairs.push((key.to_string(), value.to_string()));
        Ok(())
    }
}

fn collect_kv(record: &Record) -> KvCollector {
    let mut collector = KvCollector::new();
    let _ = record.key_values().visit(&mut collector);
    collector
}

fn format_value(v: &str) -> String {
    if v.contains(' ') {
        format!("\"{}\"", v.replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

/// Single global formatter that produces logfmt.
/// Must be used as the global `.format()` (not per-target) because fern
/// does not preserve `key_values()` when passing records to child dispatches.
pub fn make_logfmt_formatter(
    timezone_strategy: tauri_plugin_log::TimezoneStrategy,
) -> impl Fn(FormatCallback, &Arguments, &Record) + Send + Sync + 'static {
    move |out, message, record| {
        let ts = timestamp_now(&timezone_strategy);
        let level = record.level();
        let module = record.module_path().unwrap_or("-");

        let kv = collect_kv(record);
        let mut extra = String::new();
        for (k, v) in &kv.pairs {
            extra.push(' ');
            extra.push_str(k);
            extra.push('=');
            extra.push_str(&format_value(v));
        }

        let msg = format!("{}", message);
        let msg_fmt = format_value(&msg);

        out.finish(format_args!(
            "ts={ts} level={level} module={module}{extra} msg={msg_fmt}"
        ));
    }
}
