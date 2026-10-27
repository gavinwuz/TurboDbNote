//! Account metadata excludes authentication fields; creation uses frozen, confirmed inputs.
use super::{Access, DatabaseKind, Reply, metadata_literal};
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct Draft {
    pub user: String,
    pub host: String,
    pub password: String,
}
impl std::fmt::Debug for Draft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountDraft")
            .field("user", &self.user)
            .field("host", &self.host)
            .field("password", &"***")
            .finish()
    }
}
pub fn supported(kind: DatabaseKind, version: &str, create: bool) -> bool {
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|part| part.split('-').next())
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    kind == DatabaseKind::MySql
        && if create {
            major > 5 || major == 5 && (minor > 0 || patch >= 2)
        } else {
            major >= 5
        }
}
fn valid_identity(user: &str, host: &str) -> bool {
    user.chars().count() <= 32
        && !host.is_empty()
        && host.len() <= 255
        && !user.chars().chain(host.chars()).any(char::is_control)
}
fn quoted(value: &str, mode: &str) -> String {
    let value = if mode
        .split(',')
        .any(|m| m.trim().eq_ignore_ascii_case("NO_BACKSLASH_ESCAPES"))
    {
        value.to_owned()
    } else {
        value.replace('\\', "\\\\")
    };
    format!("'{}'", value.replace('\'', "''"))
}
pub fn create_sql(draft: &Draft, mode: &str, masked: bool) -> Result<String, String> {
    if draft.user.is_empty()
        || !valid_identity(&draft.user, &draft.host)
        || draft.password.is_empty()
        || draft.password.len() > 1024
        || draft.password.contains('\0')
        || mode.len() > 4096
    {
        return Err("accounts.invalid".into());
    }
    Ok(format!(
        "CREATE USER {}@{} IDENTIFIED BY {};",
        quoted(&draft.user, mode),
        quoted(&draft.host, mode),
        quoted(if masked { "***" } else { &draft.password }, mode)
    ))
}
pub async fn mode(c: &mut impl Access, kind: DatabaseKind, version: &str) -> Result<Reply, String> {
    if !supported(kind, version, true) {
        return Err("accounts.unsupported".into());
    }
    let rows = c.query("SELECT @@SESSION.sql_mode AS sql_mode").await?;
    if rows
        .values
        .first()
        .and_then(|r| r.first())
        .and_then(Option::as_ref)
        .is_none()
    {
        return Err("accounts.prepare_failed".into());
    }
    Ok(Reply::Page {
        rows,
        offset: 0,
        more: false,
        sql: String::new(),
    })
}
pub async fn create(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    draft: &Draft,
    mode: &str,
    confirmed: bool,
) -> Result<Reply, String> {
    if !confirmed {
        return Err("accounts.confirm_required".into());
    }
    if !supported(kind, version, true) {
        return Err("accounts.unsupported".into());
    }
    let sql = create_sql(draft, mode, false)?;
    let current = c.query("SELECT @@SESSION.sql_mode AS sql_mode").await?;
    if current
        .values
        .first()
        .and_then(|r| r.first())
        .and_then(Option::as_deref)
        != Some(mode)
    {
        return Err("accounts.stale_mode".into());
    }
    // No IF NOT EXISTS, GRANT, authentication-plugin overrides, or automatic retry.
    c.query(&sql)
        .await
        .map_err(|_| "accounts.create_failed".to_owned())?;
    Ok(Reply::Executed)
}
pub async fn permissions(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    user: &str,
    host: &str,
    search: &str,
    offset: usize,
) -> Result<Reply, String> {
    if !supported(kind, version, false) {
        return Err("accounts.unsupported".into());
    }
    if !valid_identity(user, host) || search.len() > 512 || search.contains('\0') || offset > 20_000
    {
        return Err("accounts.invalid".into());
    }
    let grantee = metadata_literal(kind, &format!("'{user}'@'{host}'"));
    let source = format!(
        "SELECT 'global' AS scope,'*' AS db_name,'*' AS object_name,'' AS column_name,PRIVILEGE_TYPE AS privilege_name,IS_GRANTABLE AS grantable FROM information_schema.USER_PRIVILEGES WHERE GRANTEE={grantee} UNION ALL SELECT 'database',TABLE_SCHEMA,'*','',PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.SCHEMA_PRIVILEGES WHERE GRANTEE={grantee} UNION ALL SELECT 'table',TABLE_SCHEMA,TABLE_NAME,'',PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.TABLE_PRIVILEGES WHERE GRANTEE={grantee} UNION ALL SELECT 'column',TABLE_SCHEMA,TABLE_NAME,COLUMN_NAME,PRIVILEGE_TYPE,IS_GRANTABLE FROM information_schema.COLUMN_PRIVILEGES WHERE GRANTEE={grantee}"
    );
    let predicate = if search.is_empty() {
        String::new()
    } else {
        format!(
            " WHERE LOCATE({},LOWER(CONCAT_WS(' ',scope,db_name,object_name,column_name,privilege_name)))>0",
            metadata_literal(kind, &search.to_lowercase())
        )
    };
    let mut rows=c.query(&format!("SELECT * FROM ({source}) account_privileges{predicate} ORDER BY scope,db_name,object_name,column_name,privilege_name LIMIT 201 OFFSET {offset}")).await?;
    let more = rows.values.len() > 200;
    rows.values.truncate(200);
    // Only this explicit projection may reach the UI. Never SHOW CREATE USER / raw SHOW GRANTS.
    rows.columns = vec![
        "scope",
        "database",
        "object",
        "column",
        "privilege",
        "grantable",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    Ok(Reply::Page {
        rows,
        offset,
        more,
        sql: String::new(),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Grant {
    pub scope: String,
    pub database: String,
    pub object: String,
    pub column: String,
    pub privilege: String,
    pub grantable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrantPlan {
    pub user: String,
    pub host: String,
    pub before: Vec<Grant>,
    pub after: Vec<Grant>,
}
pub fn privileges(scope: &str) -> &'static [&'static str] {
    match scope {
        "column" => &["SELECT", "INSERT", "UPDATE", "REFERENCES"],
        "table" => &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "INDEX",
            "REFERENCES",
            "CREATE VIEW",
            "SHOW VIEW",
            "TRIGGER",
        ],
        "database" => &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "INDEX",
            "REFERENCES",
            "CREATE VIEW",
            "SHOW VIEW",
            "CREATE ROUTINE",
            "ALTER ROUTINE",
            "EXECUTE",
            "CREATE TEMPORARY TABLES",
            "LOCK TABLES",
            "EVENT",
            "TRIGGER",
        ],
        "global" => &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "INDEX",
            "REFERENCES",
            "CREATE VIEW",
            "SHOW VIEW",
            "CREATE ROUTINE",
            "ALTER ROUTINE",
            "EXECUTE",
            "CREATE TEMPORARY TABLES",
            "LOCK TABLES",
            "EVENT",
            "TRIGGER",
            "CREATE USER",
            "RELOAD",
            "PROCESS",
            "SHOW DATABASES",
            "REPLICATION CLIENT",
            "REPLICATION SLAVE",
            "FILE",
            "SUPER",
            "SHUTDOWN",
        ],
        _ => &[],
    }
}
fn grant_scope(grant: &Grant) -> Result<String, String> {
    let valid = |s: &str| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
    let quote = |s: &str| super::quote(DatabaseKind::MySql, s);
    match grant.scope.as_str() {
        "global" if grant.database == "*" && grant.object == "*" && grant.column.is_empty() => {
            Ok("*.*".into())
        }
        "database" if valid(&grant.database) && grant.object == "*" && grant.column.is_empty() => {
            Ok(format!("{}.*", quote(&grant.database)))
        }
        "table" if valid(&grant.database) && valid(&grant.object) && grant.column.is_empty() => Ok(
            format!("{}.{}", quote(&grant.database), quote(&grant.object)),
        ),
        "column" if valid(&grant.database) && valid(&grant.object) && valid(&grant.column) => Ok(
            format!("{}.{}", quote(&grant.database), quote(&grant.object)),
        ),
        _ => Err("accounts.grants.invalid".into()),
    }
}
/// MySQL database-level grants use LIKE patterns even inside backticks.
/// Keep this escaped representation in Grant so previews and subsequent REVOKE agree.
pub fn literal_database_scope(database: &str) -> String {
    database
        .replace('\\', "\\\\")
        .replace('_', "\\_")
        .replace('%', "\\%")
}
fn literal_pattern(database: &str) -> bool {
    let mut chars = database.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '_' | '%' => return false,
            '\\' => {
                if !matches!(chars.next(), Some('\\' | '_' | '%')) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}
pub fn database_scope_label(database: &str) -> String {
    if !literal_pattern(database) {
        return database.to_owned();
    }
    let mut chars = database.chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}
pub fn grant_sql(plan: &GrantPlan) -> Result<Vec<String>, String> {
    if !valid_identity(&plan.user, &plan.host)
        || plan.before.len() > 2000
        || plan.after.len() > 2000
    {
        return Err("accounts.grants.invalid".into());
    }
    // New schema-pattern grants must not silently widen literal names containing LIKE wildcards.
    for grant in &plan.after {
        if grant.scope == "database"
            && !literal_pattern(&grant.database)
            && !plan
                .before
                .iter()
                .any(|before| before.scope == grant.scope && before.database == grant.database)
        {
            return Err("accounts.grants.pattern".into());
        }
        if grant.scope == "column" && !plan.before.contains(grant) && grant.grantable {
            return Err("accounts.grants.column_option".into());
        }
    }
    let account = format!(
        "{}@{}",
        super::quote(DatabaseKind::MySql, &plan.user),
        super::quote(DatabaseKind::MySql, &plan.host)
    );
    let mut sql = Vec::new();
    let mut revoke_options = std::collections::BTreeSet::new();
    for grant in &plan.before {
        let target = grant_scope(grant)?;
        if grant.grantable
            && !plan
                .after
                .iter()
                .any(|after| grant_scope(after).ok().as_ref() == Some(&target) && after.grantable)
        {
            revoke_options.insert(target);
        }
    }
    for target in revoke_options {
        sql.push(format!("REVOKE GRANT OPTION ON {target} FROM {account};"));
    }
    for (grants, other, revoke) in [
        (&plan.before, &plan.after, true),
        (&plan.after, &plan.before, false),
    ] {
        for grant in grants {
            let unchanged = other.iter().any(|g| {
                g.scope == grant.scope
                    && g.database == grant.database
                    && g.object == grant.object
                    && g.column == grant.column
                    && g.privilege == grant.privilege
                    && (revoke || g.grantable == grant.grantable)
            });
            if unchanged {
                continue;
            }
            if !privileges(&grant.scope).contains(&grant.privilege.as_str()) {
                return Err("accounts.grants.invalid".into());
            }
            let target = grant_scope(grant)?;
            let privilege = if grant.column.is_empty() {
                grant.privilege.clone()
            } else {
                format!(
                    "{} ({})",
                    grant.privilege,
                    super::quote(DatabaseKind::MySql, &grant.column)
                )
            };
            sql.push(if revoke {
                format!("REVOKE {privilege} ON {target} FROM {account};")
            } else {
                format!(
                    "GRANT {privilege} ON {target} TO {account}{};",
                    if grant.grantable {
                        " WITH GRANT OPTION"
                    } else {
                        ""
                    }
                )
            });
        }
    }
    if sql.len() > 128 {
        return Err("accounts.grants.limit".into());
    }
    Ok(sql)
}
pub async fn grants(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    user: &str,
    host: &str,
) -> Result<Vec<Grant>, String> {
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let Reply::Page { rows, more, .. } =
            permissions(c, kind, version, user, host, "", offset).await?
        else {
            return Err("accounts.read_failed".into());
        };
        for row in rows.values {
            let val = |n: usize| row.get(n).and_then(Clone::clone).unwrap_or_default();
            all.push(Grant {
                scope: val(0),
                database: val(1),
                object: val(2),
                column: val(3),
                privilege: val(4),
                grantable: val(5) == "YES",
            });
        }
        if all.len() > 2000 || more && all.len() >= 2000 {
            return Err("accounts.grants.limit".into());
        }
        if !more {
            break;
        }
        offset += 200;
    }
    all.sort();
    all.dedup();
    Ok(all)
}
pub async fn apply_grants(
    c: &mut impl Access,
    kind: DatabaseKind,
    version: &str,
    plan: &GrantPlan,
    confirmed: bool,
) -> Result<Reply, String> {
    if !confirmed {
        return Err("accounts.confirm_required".into());
    }
    let sql = grant_sql(plan)?;
    let mut before = plan.before.clone();
    before.sort();
    before.dedup();
    if grants(c, kind, version, &plan.user, &plan.host).await? != before {
        return Err("accounts.grants.stale".into());
    }
    for statement in sql {
        let check = format!(
            "SELECT User,Host FROM mysql.user WHERE User={} AND Host={}",
            metadata_literal(kind, &plan.user),
            metadata_literal(kind, &plan.host)
        );
        if c.query(&check).await?.values.len() != 1 {
            return Err("accounts.grants.stale".into());
        }
        c.query(&statement)
            .await
            .map_err(|_| "accounts.grants.failed".to_owned())?;
    }
    Ok(Reply::Executed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::workbench::{Query, Rows};
    struct Fixture {
        calls: Vec<String>,
        mode: String,
        fail: bool,
    }
    impl Access for Fixture {
        fn query<'a>(&'a mut self, sql: &'a str) -> Query<'a> {
            self.calls.push(sql.to_owned());
            Box::pin(async move {
                if self.fail {
                    return Err("synthetic permission failure".into());
                }
                if sql == "SELECT VERSION() AS version" {
                    return Ok(Rows {
                        columns: vec!["version".into()],
                        values: vec![vec![Some("8.0.36".into())]],
                    });
                }
                if sql.starts_with("SELECT @@SESSION.sql_mode") {
                    return Ok(Rows {
                        columns: vec!["sql_mode".into()],
                        values: vec![vec![Some(self.mode.clone())]],
                    });
                }
                Ok(Rows {
                    columns: vec![],
                    values: vec![],
                })
            })
        }
    }
    fn draft() -> Draft {
        Draft {
            user: "reader'\\name".into(),
            host: "localhost".into(),
            password: "sample'\\credential".into(),
        }
    }
    #[test]
    fn mysql_account_capabilities_include_legacy_create_user_without_auth_overrides() {
        assert!(!supported(DatabaseKind::MySql, "5.0.1", true));
        assert!(supported(DatabaseKind::MySql, "5.0.2", true));
        assert!(supported(DatabaseKind::MySql, "5.5.62", true));
        assert!(supported(DatabaseKind::MySql, "5.5.5-10.6.1-MariaDB", true));
        assert!(!supported(DatabaseKind::MySql, "4.1.22", true));
        assert!(!supported(DatabaseKind::PostgreSql, "16.0", true));
    }
    #[test]
    fn secrets_are_masked_and_mysql_modes_quote_exact_frozen_inputs() {
        let draft = draft();
        let debug = format!("{draft:?}");
        assert!(!debug.contains(&draft.password));
        assert!(debug.contains("***"));
        let masked = create_sql(&draft, "", true).unwrap();
        assert!(!masked.contains(&draft.password));
        assert!(masked.ends_with("IDENTIFIED BY '***';"));
        assert_eq!(quoted("a'\\b", ""), "'a''\\\\b'");
        assert_eq!(quoted("a'\\b", "NO_BACKSLASH_ESCAPES"), "'a''\\b'");
        assert!(
            create_sql(
                &Draft {
                    user: "x\n".into(),
                    ..draft.clone()
                },
                "",
                false
            )
            .is_err()
        );
        assert!(
            create_sql(
                &Draft {
                    password: String::new(),
                    ..draft
                },
                "",
                false
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn creation_requires_confirmation_and_unchanged_mode_and_never_retries() {
        let mut fixture = Fixture {
            calls: vec![],
            mode: "NO_BACKSLASH_ESCAPES".into(),
            fail: false,
        };
        let draft = draft();
        assert!(
            create(
                &mut fixture,
                DatabaseKind::MySql,
                "8.0.36",
                &draft,
                "",
                false
            )
            .await
            .is_err()
        );
        assert!(fixture.calls.is_empty());
        assert!(
            create(
                &mut fixture,
                DatabaseKind::MySql,
                "4.1.22",
                &draft,
                "",
                true
            )
            .await
            .is_err()
        );
        assert!(fixture.calls.is_empty());
        assert!(
            create(
                &mut fixture,
                DatabaseKind::MySql,
                "8.0.36",
                &draft,
                "",
                true
            )
            .await
            .is_err()
        );
        assert_eq!(fixture.calls.len(), 1);
        fixture.calls.clear();
        assert!(matches!(
            create(
                &mut fixture,
                DatabaseKind::MySql,
                "8.0.36",
                &draft,
                "NO_BACKSLASH_ESCAPES",
                true
            )
            .await,
            Ok(Reply::Executed)
        ));
        assert_eq!(fixture.calls.len(), 2);
        assert!(fixture.calls[1].starts_with("CREATE USER "));
        assert!(!fixture.calls[1].contains("IF NOT EXISTS"));
        fixture.fail = true;
        fixture.calls.clear();
        assert!(
            create(
                &mut fixture,
                DatabaseKind::MySql,
                "8.0.36",
                &draft,
                "NO_BACKSLASH_ESCAPES",
                true
            )
            .await
            .is_err()
        );
        assert_eq!(fixture.calls.len(), 1);
    }
    #[tokio::test]
    async fn workbench_uses_actual_server_version_instead_of_auto_target_hint() {
        let mut fixture = Fixture {
            calls: vec![],
            mode: String::new(),
            fail: false,
        };
        let request = super::super::Request {
            database: "fixture".into(),
            action: super::super::Action::AccountMode,
        };
        assert!(matches!(
            super::super::run(&mut fixture, DatabaseKind::MySql, "Auto", &request).await,
            Ok(Reply::Page { .. })
        ));
        assert_eq!(fixture.calls.len(), 2);
        assert!(
            super::super::run(&mut fixture, DatabaseKind::Sqlite, "", &request)
                .await
                .is_err()
        );
        assert_eq!(fixture.calls.len(), 2);
    }
    #[tokio::test]
    async fn privilege_projection_excludes_auth_material_and_handles_search_as_literal() {
        let mut fixture = Fixture {
            calls: vec![],
            mode: String::new(),
            fail: false,
        };
        let reply = permissions(
            &mut fixture,
            DatabaseKind::MySql,
            "5.7.44",
            "o'hare@extra",
            "%",
            "x' OR 1=1 --",
            200,
        )
        .await
        .unwrap();
        assert!(matches!(
            reply,
            Reply::Page {
                offset: 200,
                more: false,
                ..
            }
        ));
        assert_eq!(fixture.calls.len(), 1);
        let sql = &fixture.calls[0];
        assert!(
            !sql.contains("password")
                && !sql.contains("authentication")
                && !sql.contains("SHOW GRANTS")
        );
        assert!(!sql.contains("x' OR 1=1 --"));
        assert!(sql.ends_with("LIMIT 201 OFFSET 200"));
        assert!(
            permissions(
                &mut fixture,
                DatabaseKind::PostgreSql,
                "16",
                "user",
                "host",
                "",
                0
            )
            .await
            .is_err()
        );
        fixture.fail = true;
        assert!(
            permissions(
                &mut fixture,
                DatabaseKind::MySql,
                "8.0.36",
                "reader",
                "localhost",
                "",
                0
            )
            .await
            .is_err()
        );
    }
}

#[cfg(test)]
mod grant_tests {
    use super::*;
    fn grant(privilege: &str) -> Grant {
        Grant {
            scope: "table".into(),
            database: "db`name".into(),
            object: "table;name".into(),
            column: String::new(),
            privilege: privilege.into(),
            grantable: false,
        }
    }
    #[test]
    fn staged_grants_quote_identifiers_whitelist_privileges_and_revoke_only_exact_changes() {
        let plan = GrantPlan {
            user: "u@name".into(),
            host: "%".into(),
            before: vec![grant("SELECT")],
            after: vec![grant("UPDATE")],
        };
        let sql = grant_sql(&plan).unwrap();
        assert_eq!(sql.len(), 2);
        assert!(sql[0].starts_with("REVOKE SELECT ON `db``name`.`table;name` FROM `u@name`@`%`"));
        assert!(sql[1].starts_with("GRANT UPDATE"));
        assert!(
            grant_sql(&GrantPlan {
                after: vec![grant("SELECT; DROP USER")],
                ..plan.clone()
            })
            .is_err()
        );
        let mut option = plan.before[0].clone();
        option.grantable = true;
        assert!(
            grant_sql(&GrantPlan {
                before: vec![option],
                after: plan.before.clone(),
                ..plan
            })
            .unwrap()[0]
                .starts_with("REVOKE GRANT OPTION")
        );
    }
    #[test]
    fn new_database_patterns_and_column_grant_option_do_not_silently_expand_scope() {
        let mut item = grant("SELECT");
        item.scope = "database".into();
        item.database = "account_center".into();
        item.object = "*".into();
        let plan = GrantPlan {
            user: "reader".into(),
            host: "localhost".into(),
            before: vec![],
            after: vec![item],
        };
        assert!(grant_sql(&plan).is_err());
        let mut item = grant("SELECT");
        item.scope = "column".into();
        item.column = "name".into();
        item.grantable = true;
        assert!(
            grant_sql(&GrantPlan {
                after: vec![item],
                ..plan
            })
            .is_err()
        );
    }
}

#[cfg(test)]
mod apply_grant_tests {
    use super::*;
    use crate::database::workbench::{Query, Rows};
    struct Fixture {
        calls: Vec<String>,
        changed: bool,
        fail: bool,
    }
    impl Access for Fixture {
        fn query<'a>(&'a mut self, sql: &'a str) -> Query<'a> {
            self.calls.push(sql.into());
            Box::pin(async move {
                if sql.contains("account_privileges") {
                    return Ok(Rows {
                        columns: vec![],
                        values: if self.changed {
                            vec![vec![
                                Some("global".into()),
                                Some("*".into()),
                                Some("*".into()),
                                Some("".into()),
                                Some("SELECT".into()),
                                Some("NO".into()),
                            ]]
                        } else {
                            vec![]
                        },
                    });
                }
                if sql.starts_with("SELECT User,Host") {
                    return Ok(Rows {
                        columns: vec![],
                        values: vec![vec![Some("reader".into()), Some("localhost".into())]],
                    });
                }
                if self.fail {
                    return Err("synthetic failure".into());
                }
                Ok(Rows::default())
            })
        }
    }
    #[tokio::test]
    async fn apply_compares_original_snapshot_and_stops_without_retry_on_first_failure() {
        let plan = GrantPlan {
            user: "reader".into(),
            host: "localhost".into(),
            before: vec![],
            after: vec![Grant {
                scope: "table".into(),
                database: "db".into(),
                object: "items".into(),
                column: String::new(),
                privilege: "SELECT".into(),
                grantable: false,
            }],
        };
        let mut fixture = Fixture {
            calls: vec![],
            changed: true,
            fail: false,
        };
        assert!(
            apply_grants(&mut fixture, DatabaseKind::MySql, "8.0.36", &plan, false)
                .await
                .is_err()
        );
        assert!(fixture.calls.is_empty());
        assert!(
            apply_grants(&mut fixture, DatabaseKind::MySql, "8.0.36", &plan, true)
                .await
                .is_err()
        );
        assert!(fixture.calls.iter().all(|sql| !sql.starts_with("GRANT")));
        fixture.calls.clear();
        fixture.changed = false;
        fixture.fail = true;
        assert!(
            apply_grants(&mut fixture, DatabaseKind::MySql, "8.0.36", &plan, true)
                .await
                .is_err()
        );
        assert_eq!(
            fixture
                .calls
                .iter()
                .filter(|sql| sql.starts_with("GRANT"))
                .count(),
            1
        );
    }
}
