-- Phase 6 (Decisions 12 and 23): a reply's tool calls are kept with it.
--
-- `ai_messages.parts` is a JSON list of the reply's rounds: `{round, type:
-- "text", text}` and `{round, type: "tool", callId, name, input, ok,
-- result, resultBytes?}`, each tool result cut at 16 KiB. Core writes it
-- with the reply at the end of a turn, so a later turn's history can send
-- the calls and results back. NULL means "no tool calls": a user message, a
-- reply that made none, and every row written before this migration or by
-- an older release.
--
-- Expand-only (see README.md): one nullable column with no default. Older
-- releases never read it (their loads read columns by name), and their
-- puts name their own columns, so a row they write gets NULL.

ALTER TABLE ai_messages ADD COLUMN parts TEXT;
