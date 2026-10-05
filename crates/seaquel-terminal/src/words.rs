//! Words both terminal binaries use for Core's answers.

use seaquel_core::sql::statements::DestructiveReason;

/// The `SHA256:…` fingerprint in an `UNKNOWN_HOST_KEY` message: what
/// follows the last `Fingerprint: ` (Core's label), the base64 after
/// `SHA256:` only, since Core may wrap the SSH layer's message (a `)`).
/// An earlier `SHA256:` (in a host name) never counts. (A structured field
/// is a follow-up.)
pub fn host_key_fingerprint(message: &str) -> Option<String> {
    let label = "Fingerprint: SHA256:";
    let at = message.rfind(label)? + label.len();
    let rest = &message[at..];
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
        .unwrap_or(rest.len());
    (len > 0).then(|| format!("SHA256:{}", &rest[..len]))
}

/// What a destructive statement does, in a few words.
pub fn destructive_reason(reason: DestructiveReason) -> &'static str {
    use DestructiveReason as R;
    match reason {
        R::DropTable => "drops a table",
        R::DropIndex => "drops an index",
        R::DropView => "drops a view",
        R::DropSchema => "drops a schema",
        R::DropDatabase => "drops a database",
        R::DropSequence => "drops a sequence",
        R::DropFunction => "drops a function",
        R::DropColumn => "drops a column",
        R::Truncate => "empties a table",
        R::DeleteNoWhere => "DELETE without WHERE",
        R::UpdateNoWhere => "UPDATE without WHERE",
        R::MergeDelete => "MERGE that deletes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_comes_from_core_s_message() {
        let message = "The host key for 127.0.0.1:2222 is not in known_hosts.\n\
                       Fingerprint: SHA256:abcDEF123+/xyz";
        assert_eq!(
            host_key_fingerprint(message).as_deref(),
            Some("SHA256:abcDEF123+/xyz")
        );
        assert_eq!(host_key_fingerprint("no fingerprint here"), None);
        // Core wraps the SSH layer's message: what follows isn't part of it.
        assert_eq!(
            host_key_fingerprint("SSH tunnel failed (Fingerprint: SHA256:xBaw+q/E=)").as_deref(),
            Some("SHA256:xBaw+q/E=")
        );
        assert_eq!(host_key_fingerprint("Fingerprint: SHA256:"), None);
        // Only what follows "Fingerprint: " counts: an earlier SHA256: (a
        // host or user name could hold one) doesn't.
        assert_eq!(
            host_key_fingerprint(
                "host SHA256:decoy is not in known_hosts.\nFingerprint: SHA256:real"
            )
            .as_deref(),
            Some("SHA256:real")
        );
        assert_eq!(host_key_fingerprint("SHA256:alone, no label"), None);
        assert_eq!(host_key_fingerprint("Fingerprint: MD5:aa"), None);
    }

    #[test]
    fn a_destructive_reason_is_worded() {
        assert_eq!(
            destructive_reason(DestructiveReason::Truncate),
            "empties a table"
        );
        assert_eq!(
            destructive_reason(DestructiveReason::DeleteNoWhere),
            "DELETE without WHERE"
        );
    }
}
