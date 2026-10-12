# TurboDBNote Privacy Policy

Last updated: 2026-10-12

[简体中文](PRIVACY.zh-CN.md)

This policy describes the TurboDBNote desktop application maintained by [gavinwuz](https://github.com/gavinwuz), including optional integrations. Actual network activity depends on the features, configuration and third-party components in your installed build.

## Local data

TurboDBNote stores notes and drafts in standalone `.note` files. Settings, connection metadata, project/navigation state and note indexes are stored locally, including in `connections.db`. Chat history, operation logs and diagnostics may be stored separately in the application's data directories. Notes, query results, chat history and exports can contain personal or confidential information; protect your files and backups.

Saved Windows database passwords are encrypted using current-user DPAPI. Linux database passwords are held in session memory. Encryption of saved Windows passwords does not encrypt all application data. Configured AI credentials and external CLI credentials have their own storage mechanisms; do not assume every credential or document is encrypted. The application does not require a TurboDBNote cloud account or automatically synchronize notes to a maintainer-hosted cloud service.

## Connections and third-party services

Database connections send authentication information, queries and requested operations to the database servers you configure. Configured automatic connection may initiate connections at startup. SQL writes and transfers affect the selected servers and files.

When you use AI features, messages and context supplied to the session may be sent through your configured provider or installed Codex/Claude Code CLI. Those tools may access files or services according to their own settings and permissions. Provider model-list requests also contact the configured endpoint and may use your API credentials. Each provider or CLI's own privacy policy applies; avoid supplying confidential data without appropriate authorization.

Update checks contact GitHub's release API. Opening documentation or download links contacts the corresponding websites. Recipients may receive ordinary request metadata such as IP address, application version/user agent and request time. GitHub's [privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement) applies to GitHub services.

## Diagnostics and crash reporting

The built-in configuration enables local diagnostic recording. Allowlisted usage events include application start/stop, opening settings/projects/connections, a chat-sent event, agent selection and an update-check event. Records include generated event/session identifiers, version, timestamps and elapsed time; the chat-sent event does not contain chat text. Operation logging records task identifiers, stages, timing, error categories and numeric counts. Application-controlled structured events are designed to exclude passwords, keys, connection strings, SQL bodies and chat bodies.

The built-in configuration has no usage-analytics upload endpoint. If a deployment configures one, queued usage events can be transmitted to that configured HTTPS endpoint. The operator of that deployment must disclose the recipient and relevant retention arrangements.

If the installed build includes a supported native CrashSight SDK and diagnostics are enabled, the application attempts to initialize it for crash reporting. SDK initialization and actual report delivery are distinct; delivery has not been verified for the project's current release process. Crash reports may contain technical environment information, stack traces, local paths and memory dumps. Native dumps must not be treated as sanitized and can contain sensitive runtime data. The SDK may transmit reports automatically according to its implementation. Region selection follows the deployment configuration; do not assume a particular data-residency guarantee.

CrashSight is provided by Shenzhen Tencent Computer System Co., Ltd. See its [personal information protection rules](https://crashsight.wetest.net/documents/en/crashsight/privacyPolicy/) for its data processing practices. This policy does not claim that every package includes the SDK.

## Your controls and retention

You can choose connection and AI targets, disable configured automatic connections, stop active work and remove saved connections or histories using the available features. Your database/AI operators control retention of data received by their services.

To disable the diagnostics module before startup, close the application (including its tray instance), create or update `crashsight.local.json` beside the executable with the following contents, then restart:

```json
{"enabled": false}
```

This disables the diagnostic module, including local usage recording and native crash capture, but does not disable the separate operation logs. Do not overwrite an existing deployment override without retaining any settings you need. An administrator-managed installation may require administrator assistance to change that file.

Local notes, settings and histories remain until removed by you or managed by their storage/rotation rules. Diagnostic queues and operation logs have bounded/rotating storage; this is not a guarantee of a fixed deletion date. Disabling diagnostics does not remove existing files or previously submitted reports. Installation removal preserves user data. Back up what you need and, with the app closed, remove the relevant files to erase local copies. Remote deletion requests must go to the relevant service operator; no universal remote-deletion mechanism is provided by TurboDBNote.

## Contact and policy changes

For privacy questions, contact the maintainer through the [project's GitHub Issues](https://github.com/gavinwuz/TurboDbNote/issues). Issues are public: do not post passwords, API keys, private database records, private chat content or crash dumps. Request a suitable private contact channel before sharing sensitive details.

Changes to this policy are recorded in the repository history and reflected in the date above. Review the policy when updating or enabling additional integrations.
