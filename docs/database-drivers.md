# Database driver catalog and isolated runtime

## Decision

Driver identity remains separate from wire protocol. MariaDB, TiDB and Doris reuse the existing Rust MySQL adapter. Other adapters run on demand in a disposable process; the GUI has no dependency on the new driver crate. Rust static dependencies are linked into the worker, not loaded as unstable Rust ABI plugins. This is process-level unloading, not DLL hot unloading.

The worker is started for a connection, not during catalog display. Disconnect, cancellation, test completion or failure closes the process tree. There is a limit of eight external sessions; tab switches do not disconnect sessions. No Python or Java runtime is distributed. H2/JDBC is removed from the supported catalog. Native clients may allocate memory in their worker; total memory must be measured across the process tree.

## Capabilities

| Database | Implementation | Requirements and boundary |
| --- | --- | --- |
| MariaDB, TiDB, Doris | Existing Rust sqlx MySQL protocol | Existing capabilities; Doris remains read-only |
| SQL Server | Rust Tiberius TDS | No ODBC dependency; SQL or Windows integrated authentication; paging needs SQL Server 2012+ |
| MongoDB | Official Rust mongodb | admin authentication database; bounded Extended JSON browsing |
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

Protocol frames are limited to 1 MiB, aggregate preview content to 768 KiB, cells to 64 KiB, columns to 512 and page size to 1–1000. ODBC wide-character reads and Rust binary reads use bounded buffers; MongoDB uses batches of one and Cassandra keeps at most 64 paging states. SDKs can still allocate protocol buffers before application limits are applied; process isolation and termination provide recovery, not a strict OS memory quota.

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
