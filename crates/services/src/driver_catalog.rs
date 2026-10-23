//! Stable driver identities are separate from wire protocols and UI grouping.
use crate::{DatabaseKind, mysql::Profile};

#[derive(Clone, Copy)]
pub struct DriverSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: DatabaseKind,
    pub port: u16,
    pub groups: &'static [&'static str],
    pub popular: bool,
    pub file: bool,
    pub component: &'static str,
}
macro_rules! driver {
    ($id:literal,$name:literal,$kind:ident,$port:literal,[$($group:literal),*],$popular:literal,$file:literal,$component:literal) => {
        DriverSpec { id:$id,name:$name,kind:DatabaseKind::$kind,port:$port,groups:&[$($group),*],popular:$popular,file:$file,component:$component }
    }
}
pub const DRIVERS: &[DriverSpec] = &[
    driver!("odbc", "ODBC", External, 1, ["sql"], false, false, "odbc"),
    driver!("mysql", "MySQL", MySql, 3306, ["sql"], true, false, ""),
    driver!(
        "postgresql",
        "PostgreSQL",
        PostgreSql,
        5432,
        ["sql"],
        true,
        false,
        ""
    ),
    driver!(
        "sqlite",
        "SQLite",
        Sqlite,
        1,
        ["embedded", "sql"],
        true,
        true,
        ""
    ),
    driver!(
        "sqlserver",
        "SQL Server",
        External,
        1433,
        ["sql"],
        true,
        false,
        "tds"
    ),
    driver!(
        "oracle",
        "Oracle",
        External,
        1521,
        ["sql"],
        true,
        false,
        "oci"
    ),
    driver!("mariadb", "MariaDB", MySql, 3306, ["sql"], true, false, ""),
    driver!(
        "mongodb",
        "MongoDB",
        External,
        27017,
        ["nosql"],
        true,
        false,
        "mongodb"
    ),
    driver!("redis", "Redis", Redis, 6379, ["nosql"], true, false, ""),
    driver!(
        "elasticsearch",
        "Elasticsearch",
        External,
        9200,
        ["search", "nosql", "analytics"],
        true,
        false,
        "rust-http"
    ),
    driver!(
        "clickhouse",
        "ClickHouse",
        ClickHouse,
        8443,
        ["analytics"],
        true,
        false,
        ""
    ),
    driver!(
        "tidb",
        "TiDB",
        MySql,
        4000,
        ["sql", "bigdata"],
        false,
        false,
        ""
    ),
    driver!(
        "access",
        "Microsoft Access",
        External,
        1,
        ["embedded", "sql"],
        false,
        true,
        "odbc"
    ),
    driver!(
        "cassandra",
        "Cassandra",
        External,
        9042,
        ["nosql", "bigdata"],
        false,
        false,
        "cassandra"
    ),
    driver!(
        "tdengine",
        "TDengine",
        External,
        6041,
        ["timeseries", "analytics"],
        false,
        false,
        "rust-http"
    ),
    driver!(
        "db2",
        "IBM DB2 for LUW",
        External,
        50000,
        ["sql"],
        false,
        false,
        "odbc"
    ),
    driver!(
        "doris",
        "Apache Doris",
        MySql,
        9030,
        ["analytics", "bigdata"],
        false,
        false,
        ""
    ),
];
pub const GROUPS: &[(&str, &str)] = &[
    ("all", "drivers.group.all"),
    ("popular", "drivers.group.popular"),
    ("sql", "drivers.group.sql"),
    ("nosql", "drivers.group.nosql"),
    ("analytics", "drivers.group.analytics"),
    ("embedded", "drivers.group.embedded"),
    ("timeseries", "drivers.group.timeseries"),
    ("bigdata", "drivers.group.bigdata"),
    ("search", "drivers.group.search"),
    ("graph", "drivers.group.graph"),
];
// Storage compatibility only: never offered by the picker or accepted by the worker.
const RETIRED_H2: DriverSpec = driver!("h2", "H2", External, 9092, [], false, false, "removed");
pub fn find(id: &str) -> Option<&'static DriverSpec> {
    DRIVERS
        .iter()
        .find(|d| d.id == id)
        .or_else(|| (id == "h2").then_some(&RETIRED_H2))
}
impl DriverSpec {
    pub fn in_group(&self, group: &str) -> bool {
        group == "all" || (group == "popular" && self.popular) || self.groups.contains(&group)
    }
    pub fn profile(&self) -> Profile {
        let mut p = crate::database::defaults(self.kind);
        p.driver_id = self.id.into();
        p.port = self.port;
        p.user = match self.id {
            "oracle" | "sqlserver" | "db2" | "mongodb" | "elasticsearch" | "access" | "odbc" => "",
            "cassandra" => "cassandra",
            "tdengine" => "root",
            _ => &p.user,
        }
        .into();
        if ["tidb", "doris", "mariadb"].contains(&self.id) {
            p.driver = crate::mysql::Driver::Modern;
        }
        p
    }
}
impl Profile {
    pub fn supports_mutation(&self) -> bool {
        !self.mysql_raw_gbk
            && self.kind != DatabaseKind::External
            && self.driver_id != "doris"
            && (self.kind != DatabaseKind::Sqlite || !self.sqlite_read_only)
    }
    pub fn driver_spec(&self) -> Option<&'static DriverSpec> {
        if !self.driver_id.is_empty() {
            find(&self.driver_id)
        } else {
            DRIVERS
                .iter()
                .find(|d| d.kind == self.kind && d.kind != DatabaseKind::External)
        }
    }
    pub fn driver_label(&self) -> &'static str {
        self.driver_spec().map_or(self.kind.label(), |d| d.name)
    }
    pub fn is_file_database(&self) -> bool {
        self.driver_spec().is_some_and(|d| d.file)
    }
}

pub(crate) fn check_operation(p: &Profile, op: &crate::database::Operation) -> Result<(), String> {
    use crate::database::{
        OperationKind,
        notebook_query::{GridAction, command_required},
        workbench::Action,
    };
    if p.mysql_raw_gbk {
        let read = match &op.kind {
            OperationKind::Databases
            | OperationKind::Objects { .. }
            | OperationKind::Details { .. } => true,
            OperationKind::Workbench(r) => match &r.action {
                Action::AccountGrants { .. }
                | Action::AccountMode
                | Action::AccountPermissions { .. }
                | Action::Accounts { .. }
                | Action::BrowseCatalog { .. }
                | Action::BrowseColumns(_)
                | Action::BrowseSection { .. }
                | Action::Catalog
                | Action::Describe(_)
                | Action::Ddl(_)
                | Action::NoteColumns(_)
                | Action::GridPage { .. }
                | Action::Page { .. }
                | Action::Count { .. }
                | Action::NoteExplain { .. } => true,
                Action::NotePrepare { sql } => command_required(DatabaseKind::MySql, sql)
                    .is_some_and(|k| {
                        matches!(
                            k,
                            "select"
                                | "explain"
                                | "show_create"
                                | "show_columns"
                                | "show_tables"
                                | "show_databases"
                                | "show_status"
                                | "show_variables"
                        )
                    }),
                Action::NoteExecute { plan, .. } => {
                    plan.read_only
                        && command_required(DatabaseKind::MySql, &plan.sql).is_some_and(|k| {
                            matches!(
                                k,
                                "select"
                                    | "explain"
                                    | "show_create"
                                    | "show_columns"
                                    | "show_tables"
                                    | "show_databases"
                                    | "show_status"
                                    | "show_variables"
                            )
                        })
                }
                Action::NoteGrid { plan, action, .. } => {
                    plan.read_only
                        && matches!(action, GridAction::Page { .. } | GridAction::Count { .. })
                }
                _ => false,
            },
            _ => false,
        };
        return if read {
            Ok(())
        } else {
            Err("mysql.gbk.read_only".into())
        };
    }
    if p.driver_id != "doris" {
        return Ok(());
    }
    let read = match &op.kind {
        OperationKind::Databases
        | OperationKind::Objects { .. }
        | OperationKind::Details { .. } => true,
        OperationKind::Workbench(request) => match &request.action {
            Action::AccountGrants { .. }
            | Action::AccountMode
            | Action::AccountPermissions { .. }
            | Action::Accounts { .. }
            | Action::BrowseCatalog { .. }
            | Action::BrowseColumns(_)
            | Action::BrowseSection { .. }
            | Action::Catalog
            | Action::Describe(_)
            | Action::Ddl(_)
            | Action::NoteColumns(_)
            | Action::GridPage { .. }
            | Action::Count { .. }
            | Action::Page { .. }
            | Action::NoteExplain { .. } => true,
            Action::NotePrepare { sql } => {
                command_required(DatabaseKind::MySql, sql).is_some_and(|key| {
                    matches!(
                        key,
                        "select"
                            | "explain"
                            | "show_create"
                            | "show_columns"
                            | "show_tables"
                            | "show_status"
                            | "show_variables"
                    )
                })
            }
            Action::NoteExecute { plan, .. } => plan.read_only,
            Action::NoteGrid { plan, action, .. } => {
                plan.read_only
                    && matches!(action, GridAction::Page { .. } | GridAction::Count { .. })
            }
            _ => false,
        },
        _ => false,
    };
    if read {
        Ok(())
    } else {
        Err("drivers.operation_unsupported".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identities_and_groups_preserve_protocol_compatibility() {
        let mut ids = std::collections::HashSet::new();
        for driver in DRIVERS {
            assert!(ids.insert(driver.id));
            assert!(driver.in_group("all"));
            assert_eq!(driver.profile().driver_spec().unwrap().id, driver.id);
        }
        assert_eq!(find("tidb").unwrap().kind, DatabaseKind::MySql);
        assert!(find("access").unwrap().file);
        assert!(find("tdengine").unwrap().in_group("timeseries"));
        assert_eq!(Profile::default().driver_label(), "MySQL");
        assert!(!find("doris").unwrap().profile().supports_mutation());
        assert!(!find("db2").unwrap().profile().supports_mutation());
        let request = crate::database::Operation {
            id: 1,
            kind: crate::database::OperationKind::CreateDatabase {
                name: "fixture".into(),
                charset: String::new(),
                collation: String::new(),
            },
        };
        assert!(check_operation(&find("doris").unwrap().profile(), &request).is_err());
        assert!(check_operation(&find("mariadb").unwrap().profile(), &request).is_ok());
    }
}
