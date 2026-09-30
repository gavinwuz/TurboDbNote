# SQL MCP bridge

TurboDBNote starts an ephemeral loopback-only Streamable HTTP endpoint when its AI backend connects, and closes it when that backend disconnects. `current_database` exposes only an opaque selected connection reference, database and driver kind. `query_sql` accepts a single SQL string, never connection parameters, usernames or passwords. Supported execution adapters are MySQL, PostgreSQL and SQLite.

Every SQL request is prepared through the database adapter, checked against Settings → Database → SQL, then shown in a selectable, read-only SQL review card in the chat stream. Sharing results is explicit; risky queries and writes require another confirmation in the same card. Execution rechecks the chat turn, scope, connection generation and current command policy. Cancelling a turn or switching scope before confirmation prevents execution. Already submitted writes cannot be undone by cancelling the AI turn.

The HTTP service validates the ephemeral bearer token and loopback Host, rejects browser Origin requests, and bounds body size, concurrent connections, queue length and timeout. Tokens are passed to child processes in their environment, never database credentials or command-line secret values. Codex receives a process-local MCP configuration and disables shell, other connector, hook and multi-agent features for this SQL assistant. Claude retains its built-in-tools-disabled setup and enables only the two guarded database tools. Global user configuration files are not rewritten.

Results contain at most 20 rows and 64 columns, normally 512 characters per value, with a total 32 KiB value budget. Single-value results such as EXPLAIN JSON may use that entire budget. Limits and truncation are reported. Values under common password/token/secret column names are masked; this is not a guarantee that arbitrary user data or deliberately aliased sensitive columns are anonymized. Review the requested SQL before sharing results.

The bridge is not an operating-system sandbox for third-party CLI binaries. Install trusted compatible Codex/Claude versions. A restored chat does not execute SQL automatically.

## Protocol references

- The HTTP JSON response and initialization flow follow [MCP 2025-06-18 Streamable HTTP](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports).
- Codex uses the documented [MCP bearer-token environment setting](https://developers.openai.com/codex/mcp/) and [feature overrides](https://developers.openai.com/codex/config-basic/).
- Claude uses process-local [MCP configuration and environment interpolation](https://code.claude.com/docs/en/mcp).

## Verification boundary

Tests cover authenticated HTTP initialization/tool dispatch, unauthorized and browser-origin rejection, argument restrictions, bounded UTF-8 results, redaction, SQL command classification and safe preference defaults. Database execution and native controls are validated separately with isolated fixtures. A separate explicit `sql_mcp_probe <codex.exe> --live` run with Codex 0.156.1 verified actual model calls to both tools and a real EXPLAIN against an isolated in-memory SQLite database. This does not validate production databases, MySQL/PostgreSQL MCP execution, Claude model calls, or the native approval-card workflow end-to-end.

### Tool approvals and database readiness

The process-local Codex MCP configuration explicitly approves only `current_database` and `query_sql`, and requires this server to initialize. This avoids the CLI's `never` approval policy rejecting a tool before TurboDBNote can perform its own SQL review. It does not approve shell/file operations or bypass application SQL permission, target-generation and write confirmation checks. `current_database` is annotated as read-only; `query_sql` remains potentially destructive because settings can permit writes.

Accepting a Note SQL draft readies the selected saved connection. MCP requests wait for an in-progress connection (up to 60 seconds), and discard requests if the chat turn or selected scope changes. Failed or manually disconnected profiles are not automatically retried in a loop.


### Chat access modes

The composer shows a shield and an access-mode menu. Read only is the default and permits SELECT, EXPLAIN (excluding ANALYZE), SHOW CREATE TABLE/VIEW and SHOW INDEX/INDEXES/KEYS/COLUMNS/FIELDS. Read/write adds INSERT, UPDATE and DELETE. Administrator permits all implemented command categories, including DROP and TRUNCATE. Settings command permissions, adapter validation and database grants remain additional restrictions.

Access is bound to the selected scope, database connection generation and chat backend generation. Scope changes, new chats, engine switches, disconnects and application restart reset it to read only. Mode changes reset pending confirmations and never execute SQL. A blocked card offers the mode menu and/or SQL settings entry; enabling permission still requires explicit execution approval.

Cards show frozen SQL and target, and remain in the chat stream after completion with duration, row count and expandable bounded results. Writes need a second explicit action in the same card. Cancel, stopped turns, stale targets and expired requests cannot be approved. Already submitted writes are not undone. At most eight cards are retained in memory; request controls and results are not persisted or reactivated on restart.


Native Windows verification used a real Codex session with an isolated SQLite file: a SELECT waited in the chat card, executed after approval and returned one row; INSERT was blocked in read-only mode, remained absent after switching modes and after the first confirmation, and inserted exactly one fixture row only after the second inline confirmation. An expired request did not execute. No business databases were used.
