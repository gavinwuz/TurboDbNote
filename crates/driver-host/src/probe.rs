//! Dependency checks never establish database connections.
use serde_json::json;
pub fn run(driver: &str) -> serde_json::Value {
    let reason = match driver {
        "sqlserver" | "mongodb" | "cassandra" | "elasticsearch" | "tdengine" => None,
        "oracle" => {
            let root = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("oracle")));
            let mut params = oracle::InitParams::new();
            if let Some(root) = root.filter(|p| {
                p.join(if cfg!(windows) {
                    "oci.dll"
                } else {
                    "libclntsh.so"
                })
                .is_file()
            }) {
                match params.oracle_client_lib_dir(root) {
                    Ok(_) => {}
                    Err(_) => return result(driver, Some("drivers.oracle_client_required")),
                }
            }
            if params.init().is_ok() && oracle::Version::client().is_ok() {
                None
            } else {
                Some("drivers.oracle_client_required")
            }
        }
        "odbc" => {
            if odbc_api::Environment::new()
                .and_then(|e| e.drivers())
                .is_ok_and(|d| !d.is_empty())
            {
                None
            } else {
                Some("odbc.driver_missing")
            }
        }
        "access" | "db2" => {
            let found = odbc_api::Environment::new()
                .ok()
                .and_then(|e| e.drivers().ok())
                .is_some_and(|drivers| {
                    drivers
                        .iter()
                        .any(|p| matches_driver(driver, &p.description))
                });
            if found {
                None
            } else {
                Some("drivers.odbc_missing")
            }
        }
        _ => Some("drivers.invalid_identity"),
    };
    result(driver, reason)
}
fn result(driver: &str, reason: Option<&str>) -> serde_json::Value {
    json!({"protocol":5,"app_version":env!("CARGO_PKG_VERSION"),"target":if cfg!(target_os="windows") && cfg!(target_arch="x86_64") {"windows-x64"} else if cfg!(target_os="linux") && cfg!(target_arch="x86_64") {"linux-x64"} else {"unsupported"},"driver_id":driver,"ready":reason.is_none(),"reason":reason.unwrap_or("")})
}

/// Use identical matching for readiness and real connections.
pub fn matches_driver(driver: &str, description: &str) -> bool {
    let name = description.to_ascii_lowercase();
    match driver {
        "access" => name.contains("access") && name.contains("*.accdb"),
        "db2" => name.contains("db2") || name.contains("ibm data server"),
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn providers_use_the_same_rule_for_probe_and_connect() {
        assert!(super::matches_driver(
            "access",
            "Microsoft Access Driver (*.mdb, *.accdb)"
        ));
        assert!(!super::matches_driver(
            "access",
            "Microsoft Access Driver (*.mdb)"
        ));
        assert!(super::matches_driver(
            "db2",
            "IBM DATA SERVER DRIVER for ODBC and CLI"
        ));
    }
}
