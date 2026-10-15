# Database driver catalog and isolated runtime

## Decision

Driver identity remains separate from wire protocol. MariaDB, TiDB and Doris reuse the existing Rust MySQL adapter. Other adapters run on demand in a disposable process; the GUI has no dependency on the new driver crate. Rust static dependencies are linked into the worker, not loaded as unstable Rust ABI plugins. This is process-level unloading, not DLL hot unloading.

The worker is started for a connection, not during catalog display. Disconnect, cancellation, test completion or failure closes the process tree. There is a limit of eight external sessions; tab switches do not disconnect sessions. No Python or Java runtime is distributed. H2/JDBC is removed from the supported catalog. Native clients may allocate memory in their worker; total memory must be measured across the process tree.

## Capabilities

| Database | Implementation | Requirements and boundary |
| --- | --- | --- |
| MariaDB, TiDB, Doris | Existing Rust sqlx MySQL protocol | Existing capabilities; Doris remains read-only |
| SQL Server | Rust Tiberius TDS | No ODBC dependency; SQL or Windows integrated authentication; paging needs SQL Server 2012+ |
| MongoDB | Official Rust mongodb | configurable authentication database (admin compatibility default); bounded Extended JSON browsing |
| Cassandra | Rust scylla CQL | One connection per host pool; bounded paging-state cache; no SQL emulation |
| Elasticsearch | Rust reqwest HTTP | REST JSON documents; first 10,000-result window |
| TDengine | Rust reqwest HTTP | Retains taosAdapter REST contract; not a WebSocket migration |
| Access | Rust odbc-api | Installed x64 Microsoft Access ODBC engine; existing files; read-only first page |
| DB2 LUW | Rust odbc-api | Installed x64 IBM DB2 ODBC/CLI provider; explicit plaintext only |
| Oracle | Rust oracle / ODPI-C / OCI | x64 Oracle Client; service name; TCP or identity-verified TCPS using system trust |

External adapters retain typed catalog/schema/page/count operations. They do not enable arbitrary Note SQL, AI SQL, mutations or transfers. Access is first-page-only. Metadata-only DDL previews are labelled; Oracle DBMS_METADATA still needs grants. The GUI shows worker availability, not proof of installed ODBC/OCI dependencies or server access.

Oracle Instant Client may be placed in `drivers/native/oracle` (with oci.dll and its dependencies); otherwise OCI uses its standard discovery. Unlike the previous Python Thin adapter, OCI is now required. OCI clients supporting `WALLET_LOCATION=SYSTEM` can use TCPS with `SSL_SERVER_DN_MATCH=YES`. Profile PEM/client-certificate mapping and encryption-only mode are not implemented: these requests fail explicitly and never downgrade. This descriptor path is unit-tested, not real-Oracle TLS acceptance. DB2 now needs an installed ODBC provider rather than the previous Python wheel's embedded CLI. These are migration prerequisites, not automatically fulfilled by the worker EXE.

## Build and packaging

`crates/driver-host` is a separate workspace package with no desktop dependency. Build with `cargo build --locked -j 1 -p turbodbnote-driver-host`. Preview and package scripts copy its executable into `drivers/native` beside the application. Dependencies are locked in Cargo.lock. Driver staging needs neither Python, Java nor downloads; the packaging icon verification script still uses build-time Python.

The supported catalog has 16 drivers. H2 is not selectable and has no executable adapter. Existing H2 records remain readable so a retired entry cannot prevent the whole connection library from loading; connecting fails with an explicit removed-driver message. Records and database files are not deleted automatically.

`scripts/clean-target.ps1 -ObsoleteJdbcRuntime` previews removal of old generated preview JRE/JAR/class files; add `-Apply` to remove them. It preserves the Rust host, user data and Cargo/download caches and refuses linked resources or active preview processes. Fresh distributions contain no JDBC artifacts.

## Safety and resource bounds

The existing parent transport retains bounded queues, private stdin credentials, deadlines, cancellation and Windows process-tree cleanup. No credentials go into command arguments. Workers expose only fixed error categories and do not log server errors or SQL. Unsupported operations are rejected before database context changes. HTTP disables ambient proxies and redirects. TLS options are explicit; unsupported settings fail rather than downgrade.

Protocol frames are limited to 1 MiB. Relational result frames limit aggregate preview content to 768 KiB, cells to 64 KiB, columns to 512 and page size to 1–1000. MongoDB document limits are listed below. ODBC wide-character reads and Rust binary reads use bounded buffers; MongoDB uses batches of one and Cassandra keeps at most 64 paging states. SDKs can still allocate protocol buffers before application limits are applied; process isolation and termination provide recovery, not a strict OS memory quota.

## Validation

Rust tests cover retired H2 record compatibility, worker rejection, quoting, bounded results and TLS descriptor generation. `crates/driver-host/examples/verify_contracts.rs` uses isolated Elasticsearch/TDengine HTTP contracts and checks worker exit and error redaction. These checks are not real-server acceptance for those engines or other external adapters.

Before H2 removal, native Windows GUI validation confirmed a Rust HTTP worker's 0 → 1 → 0 lifecycle on connect/disconnect and displayed bounded data/JSON. H2 test results from the earlier JDBC implementation do not imply current H2 support.

## Primary references

- [Tiberius](https://github.com/prisma/tiberius)
- [MongoDB Rust driver](https://www.mongodb.com/docs/drivers/rust/current/)
- [Scylla Rust driver / Cassandra compatibility](https://github.com/scylladb/scylla-rust-driver)
- [Rust ODBC API](https://docs.rs/odbc-api/)
- [Rust Oracle / OCI requirements](https://github.com/kubo/rust-oracle)

Oracle system-trust TCPS reference: https://docs.oracle.com/en/database/oracle/oracle-database/21/dbseg/configuring-secure-sockets-layer-authentication.html

Modern MySQL routing accepts 5.6, 5.7, 8.x and 9.x server version families; 8.4.5 is no longer rejected solely by its version. MariaDB and unknown major versions are not implicitly accepted as MySQL. Authentication/TLS checks remain unchanged. Version-routing tests are not real-server acceptance: MySQL 8.4/9.x authentication, metadata, notebook queries, guarded writes and transfer tools still need isolated server verification for the exact releases. No server or credential changes are performed automatically.

## Versioned component management

Settings → Database → Drivers exposes download, offline ZIP import, dependency detection and per-driver switches. Policies are stored in the `driver-components` namespace of connections.db; no new plaintext credential store is introduced. Existing sessions remain alive when disabled; new connection attempts are rejected at Session::start. Enabling never connects. Component readiness is separate from real-server verification.

Network acquisition uses the exact app-version GitHub Release asset and SHA256SUMS.txt. Offline import explicitly confirms executable-code provenance, then validates platform, protocol, version, bounded file counts/sizes, paths and per-file hashes. Hashes inside an offline archive are integrity metadata, not a digital signature or proof of publisher identity. Versioned directories are published through an atomic active pointer; prior files are retained for running workers. Corrupt current components fail visibly rather than silently falling back. An older app-version cache is ignored after upgrading. No automatic uninstall/pruning of potentially active files is performed.

Dependency probes run only when the driver settings page is opened or refreshed, in short-lived worker processes, without database credentials or connections. SQL Server, MongoDB, Cassandra, Elasticsearch and TDengine use Rust adapters. Oracle probes OCI initialization; Access/DB2 enumerate x64 ODBC providers. Missing providers remain unavailable and link to official installation guidance. Disabling blocks future connections while preserving existing sessions; users disconnect existing sessions explicitly.

Build-machine tooling prepares Oracle Instant Client Basic 21.23 using the official archive's pinned size and SHA256. Only native runtime libraries and license/readme materials are packaged; JDBC artifacts are omitted. Windows uses private OCI discovery next to the worker; Linux also needs libaio and unixODBC for the relevant external adapters. Access Runtime and DB2 provider installation remain system prerequisites, not automatically installed.

Windows produces existing full portable/setup artifacts plus minimal portable/setup artifacts and a `*-windows-x64-drivers.zip`. Linux produces full/minimal tarballs and a `*-linux-x64-drivers.zip`. Minimal Windows retains existing MySQL legacy compatibility components. A package-flavor marker preserves minimal/full selection on update. Complete and offline packages share the same host/client source and manifest; exact versions are mandatory. Publication remains a separate authorized action.

Commands:

```text
turbodbnote-tools prepare-oracle CACHE NATIVE-DIR windows-x64 [OFFICIAL-ZIP]
turbodbnote-tools driver-components NATIVE-DIR OUTPUT-ZIP VERSION windows-x64
```

Official Oracle archive references: [Windows](https://www.oracle.com/cn/database/technologies/instant-client/winx64-64-downloads.html), [Linux](https://www.oracle.com/ca-en/database/technologies/instant-client/linux-x86-64-downloads.html). Client/server compatibility still requires server-specific validation.

Windows verification: the official pinned Oracle Basic archive was downloaded and checked, a native offline ZIP was built, SQL Server/Oracle dependency probes passed while Access/DB2 missing-provider checks failed explicitly. An isolated native settings window imported the ZIP without restarting, retained disabled switches, and persisted an enabled SQL Server switch to connections.db. No worker remained running after dependency checks or enabling. This is dependency/UI validation, not real Oracle/SQL Server server acceptance. Full/minimal installer production and Linux runtime acceptance remain separate platform checks.

MongoDB authSource is independent of the browsing target. Missing/blank values retain admin for existing records. Connection testing pings the explicit target, or authSource when no target is provided, without listing databases. Detection requests authorizedDatabases=true; list permission denial retains a successfully tested explicit target and reports a database-list warning. No fallback authentication source, permission escalation or implicit retry is performed.

The authentication-source change introduced component protocol 2; the document workbench introduced protocol 3; independent operation deadlines now require protocol 5. Older packages are not compatible merely because the application version is unchanged.

Connection URL import is local-only and bounded to 100 lines / 64 KiB and 100 open URL drafts. Supported schemes are mysql/mariadb, postgres/postgresql, redis/rediss, clickhouse and single-host mongodb. Supported query options are MongoDB authSource, TLS tls/ssl and MySQL charset; unknown, duplicate, contradictory or malformed options reject the row without exposing input in errors. SRV and multi-host topology are not supported by this form. URI authentication semantics are frozen during parsing; an authSource-only URL suggests that name as the editable target rather than silently determining authorization. URL credentials remain in memory until each draft is explicitly saved through the existing credential transaction. New manual MongoDB forms leave the Advanced authSource override empty to follow the target (admin only when no target is entered); existing records keep their previous authSource. No authentication fallback, server connection or database listing is performed during URL import.

MongoDB document workbench uses component protocol 5. Helpers with protocol 1, 2, 3 or 4 must be replaced; the component mismatch UI reports installed versus expected metadata. The worker returns canonical EJSON pages bounded to 1000 documents / 384 KiB aggregate and 256 KiB per document. Oversized pages fail explicitly and require a smaller page size; no truncated document can be edited. The desktop projects at most 256 top-level fields in a background task, retaining page field order and raw values independently from display summaries. Mixed BSON types and missing/null are distinct. Tree nodes are virtual, depth-limited and bounded to 10,000 visible nodes, with child batches of 100. Copying uses complete canonical EJSON; Shell expressions are for copying only and never evaluated by the application.

Existing and new MongoDB profiles retain a read-only default (`mongo_read_only=true`). Users explicitly disable it under Advanced, save and reconnect before document updates/clones become available. Staged changes block paging/refresh and require explicit handling on closing. Preview freezes collection, original/replacement, identity and connection generation; a separate confirmation authorizes one mutation. The worker independently validates confirmation, read-only policy, EJSON bounds, immutable _id and actual collection type. Replacement uses _id plus an atomic whole-document $expr comparison with simple collation; changed/missing documents report a conflict. Clone drafts omit _id; MongoDB generates it on insert. Writes are never retried automatically and timeout outcomes remain uncertain. No generic MongoDB SQL execution, bulk document writes or field deletion outside this staged document workflow is provided.

Shared grid widths use a drag-only guide without notifying the data grid on mouse moves. One bounded width update occurs on drop. Column menus provide 64–1200 px input, bounded current-page fitting and reset. A serial background writer stores width-only layouts in per-object app_documents namespaces inside connections.db; document values are not stored there.

Windows verification used an isolated portable GUI and the repository loopback MongoDB wire fixture, not a real MongoDB deployment. Typed fields, Int64 precision, array expansion, node context menus, retained invalid edits, preview cancellation, confirmed update/clone with refresh, view switching and staged-close handling were exercised. A drag retained 360 px during movement, applied 479 px on drop and persisted that width; fixture find/write counters did not increase during width changes. No frame-rate guarantee or Linux GPU/desktop acceptance is implied.

Driver settings show ready entries first and place unavailable entries in a collapsed section. Readiness buttons expose per-driver reasons through tooltips and expandable details; original-vendor client links live in those details. Switches retain the existing policy transaction and affect new sessions only. The settings footer no longer forces chat navigation; tab closing and pending-settings protection remain. MySQL URL import accepts the explicit @tcp(host:port) wrapper, keeping Go DSN credentials raw. Charset is applied; Go-only interpolateParams/parseTime/loc are recognized as compatibility notices rather than silently mapped to SQL interpolation or server timezone. Rows with these notices start unselected and require explicit selection in the existing preview. Unknown/security-sensitive options remain rejected.


Db2 Rust candidates reviewed: the ibmdb organization’s rust-ibm_db / ibm_db crate and the existing third-party odbc-api adapter. rust-ibm_db still requires the IBM CLI driver, so adopting it does not resolve a missing IBM native client. Reusing odbc-api avoids a second ODBC stack; packaging/probing native IBM components and validating an actual LUW server are separate work. Sources: https://github.com/ibmdb/rust-ibm_db and https://www.ibm.com/support/pages/getting-started-ibm-data-server-drivers .


## Generic ODBC and offline Windows Db2 client

The `odbc` identity runs in the disposable Rust worker. Choose either an existing x64 DSN or an exact installed x64 driver name. Optional, non-secret attributes are entered one `KEY=VALUE` per line; the allowed keys are validated in `core::odbc::Options`. Username/password use the existing protected credential channel and are never embedded in the parameter field. Changing DSN, driver or attributes invalidates cached credential scope. DSN and driver configuration determine TLS; the app does not silently rewrite it.

The first version exposes read-only SQLTables/SQLColumns metadata and bounded SELECT previews. It does not expose arbitrary SQL, writes, Note execution or data transfer. Native driver SQL dialect support varies; common MySQL/SQL Server/Access identifier quoting is selected from the reported DBMS, with ANSI quoting as fallback. Offset pagination skips rows in the worker (maximum 100,000), retaining only one page. Ordering is not guaranteed; use a dedicated adapter for stable large-table pagination. Metadata is limited to 500 objects/512 columns and existing response byte limits.

Readiness means the worker can enumerate a local driver; it does not verify a DSN, server, password or native-driver stability. Probe and connection now use the same Access `.accdb` and Db2 matching rules. Installed Access/Office alone is not proof of x64 ODBC availability. Check **ODBC Data Sources (64-bit) → Drivers**, then refresh; a detected driver should not be reported as missing. No automatic Office reinstallation or system registration is performed.

Windows full/component packages include the unchanged IBM CLI **12.1.4 x64** archive under `drivers/native/db2-offline`, its SHA256, and bilingual installation notes. Acquire or stage it with Rust tooling:

```powershell
cargo run --locked -j 1 -p turbodbnote-tools -- prepare-db2 target/tools/db2-client target/component-verification/native
# Optional last argument: a previously downloaded original IBM ZIP.
```

Source: https://public.dhe.ibm.com/ibmdl/export/pub/software/data/db2/drivers/odbc_cli/v12.1.4/ntx64_odbc_cli.zip

SHA256: `28748e451e628c41a49676548439c980f02155890d4126342c167fed27a0d63f`

Read the included IBM licenses, extract into a permanent directory and explicitly run `db2cli.exe install -setup` from an Administrator Command Prompt in `clidriver/bin`, then refresh. The app never performs this registration automatically. Db2 LUW is the adapter target; IBM i/zOS can require separate Db2 Connect licensing. The archive has been acquired/hashed on Windows; real Db2 server compatibility has not been verified. Linux packages continue to require a platform-specific externally installed IBM client; they do not include the Windows ZIP. Worker/component protocol is now **5**, so older component ZIPs must be rebuilt/imported.

Driver rows share explicit widths; expanded details wrap below their row without resizing the name/status/switch tracks. Generated network connection names always use `database@last-host-segment`, with a numeric suffix on collisions; existing names remain unchanged.

Access/ODBC native verification: create an isolated Access file containing `sample_rows(id INTEGER, label VARCHAR(80))` seeded with IDs 1–8, then run `cargo run --locked -j 1 -p turbodbnote-driver-host --example verify_odbc -- ABSOLUTE-FIXTURE.accdb`. It uses the worker stdin/stdout protocol, checks both identities, catalog, fields, bounded pages and rejected write actions, and never emits native error text. This verifies only the local installed x64 ACE provider, not other ODBC vendors.

Connection establishment keeps its own timeout; operation deadlines are independently propagated to the Rust worker, HTTP request, Oracle call, Cassandra execution profile and parent watchdog. Installed older protocol-4 components must be updated; the IBM system driver registration remains unchanged.
