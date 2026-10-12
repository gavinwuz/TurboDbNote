use crate::protocol::*;
use futures_util::TryStreamExt;
use odbc_api::Cursor;
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio_util::compat::TokioAsyncWriteCompatExt;

pub enum Relational {
    Tds(Box<tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>>),
    Odbc(odbc_api::Connection<'static>),
    Oracle(oracle::Connection),
}
fn odbc_value(v: &str) -> Result<String> {
    if v.contains('\0') {
        return Err("drivers.invalid_target");
    }
    Ok(format!("{{{}}}", v.replace('}', "}}")))
}
fn oracle_address(p: &Value, timeout: u64) -> Result<String> {
    // OCI system trust is supported; PEM profiles must not be silently ignored.
    let tls = string(p, "tls");
    if !["Disabled", "VerifyIdentity"].contains(&tls)
        || !string(p, "ca").is_empty()
        || !string(p, "client_cert").is_empty()
    {
        return Err("drivers.tls_options_unsupported");
    }
    let host = string(p, "host");
    let db = string(p, "database");
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_alphanumeric() || ".:-[]".contains(c))
        || db.is_empty()
        || !db.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c))
    {
        return Err("drivers.invalid_target");
    }
    let protocol = if tls == "Disabled" { "TCP" } else { "TCPS" };
    let security = if tls == "Disabled" {
        ""
    } else {
        "(SECURITY=(WALLET_LOCATION=SYSTEM)(SSL_SERVER_DN_MATCH=YES))"
    };
    Ok(format!(
        "(DESCRIPTION=(TRANSPORT_CONNECT_TIMEOUT={timeout})(CONNECT_TIMEOUT={timeout})(ADDRESS=(PROTOCOL={protocol})(HOST={host})(PORT={}))(CONNECT_DATA=(SERVICE_NAME={db})){security})",
        p["port"]
    ))
}
impl Relational {
    pub async fn connect(p: &Value, password: &str) -> Result<Self> {
        let driver = string(p, "driver_id");
        let timeout = p["timeout_seconds"].as_u64().unwrap_or(10).clamp(1, 60);
        if driver == "sqlserver" {
            if !string(p, "client_cert").is_empty() {
                return Err("drivers.tls_options_unsupported");
            }
            let mut config = tiberius::Config::new();
            config.host(string(p, "host"));
            config.port(p["port"].as_u64().ok_or(FAILED)? as u16);
            config.database(if string(p, "database").is_empty() {
                "master"
            } else {
                string(p, "database")
            });
            config.authentication(if string(p, "user").is_empty() && password.is_empty() {
                #[cfg(windows)]
                {
                    tiberius::AuthMethod::Integrated
                }
                #[cfg(not(windows))]
                {
                    return Err("drivers.authentication_unsupported");
                }
            } else {
                tiberius::AuthMethod::sql_server(string(p, "user"), password)
            });
            config.encryption(if string(p, "tls") == "Disabled" {
                tiberius::EncryptionLevel::NotSupported
            } else {
                tiberius::EncryptionLevel::Required
            });
            if string(p, "tls") == "Required" {
                config.trust_cert();
            } else if !string(p, "ca").is_empty() {
                config.trust_cert_ca(string(p, "ca"));
            }
            let tcp = tokio::net::TcpStream::connect(config.get_addr())
                .await
                .map_err(|_| FAILED)?;
            tcp.set_nodelay(true).map_err(|_| FAILED)?;
            return Ok(Self::Tds(Box::new(
                tiberius::Client::connect(config, tcp.compat_write())
                    .await
                    .map_err(|_| FAILED)?,
            )));
        }
        if driver == "oracle" {
            let address = oracle_address(p, timeout)?;
            let library = std::env::current_exe()
                .map_err(|_| FAILED)?
                .parent()
                .ok_or(FAILED)?
                .join("oracle");
            if library.join("oci.dll").is_file() {
                oracle::InitParams::new()
                    .oracle_client_lib_dir(library)
                    .map_err(|_| "drivers.oracle_client_required")?
                    .init()
                    .map_err(|_| "drivers.oracle_client_required")?;
            }
            let c =
                oracle::Connection::connect(string(p, "user"), password, address).map_err(|e| {
                    if e.dpi_code() == Some(1047) {
                        "drivers.oracle_client_required"
                    } else {
                        FAILED
                    }
                })?;
            c.set_call_timeout(Some(Duration::from_secs(timeout)))
                .map_err(|_| FAILED)?;
            return Ok(Self::Oracle(c));
        }
        if !["access", "db2"].contains(&driver) {
            return Err(UNSUPPORTED);
        }
        if driver == "db2"
            && (string(p, "tls") != "Disabled"
                || !string(p, "ca").is_empty()
                || !string(p, "client_cert").is_empty())
        {
            return Err("drivers.tls_options_unsupported");
        }
        // One environment per disposable process; it outlives the single connection.
        let env = Box::leak(Box::new(
            odbc_api::Environment::new().map_err(|_| "drivers.odbc_missing")?,
        ));
        let drivers = env.drivers().map_err(|_| "drivers.odbc_missing")?;
        let mut parts = Vec::new();
        if driver == "access" {
            let provider = drivers
                .iter()
                .find(|d| {
                    d.description.contains("Access Driver") && d.description.contains("*.accdb")
                })
                .ok_or("drivers.odbc_missing")?;
            let path = Path::new(string(p, "sqlite_path"));
            if !path.is_absolute() || !path.is_file() {
                return Err("drivers.invalid_target");
            }
            parts.extend([
                ("DRIVER", provider.description.clone()),
                ("DBQ", path.to_string_lossy().into_owned()),
                ("READONLY", "1".into()),
                ("PWD", password.into()),
            ]);
        } else {
            let provider = drivers
                .iter()
                .find(|d| d.description.to_uppercase().contains("IBM DB2 ODBC DRIVER"))
                .ok_or("drivers.odbc_missing")?;
            parts.extend([
                ("DRIVER", provider.description.clone()),
                ("DATABASE", string(p, "database").into()),
                ("HOSTNAME", string(p, "host").into()),
                ("PORT", p["port"].to_string()),
                ("PROTOCOL", "TCPIP".into()),
                ("UID", string(p, "user").into()),
                ("PWD", password.into()),
            ]);
        }
        let text = zeroize::Zeroizing::new(
            parts
                .iter()
                .map(|(k, v)| Ok(format!("{k}={}", odbc_value(v)?)))
                .collect::<Result<Vec<_>>>()?
                .join(";"),
        );
        let options = odbc_api::ConnectionOptions {
            login_timeout_sec: Some(timeout as u32),
            ..Default::default()
        };
        let c = env
            .connect_with_connection_string(&text, options)
            .map_err(|_| FAILED)?;
        Ok(Self::Odbc(c))
    }
    pub async fn query(&mut self, sql: &str, maximum: usize, timeout: usize) -> Result<Rows> {
        match self {
            Self::Tds(c) => {
                let mut stream = c.simple_query(sql).await.map_err(|_| FAILED)?;
                let mut rows = Rows::default();
                while let Some(item) = stream.try_next().await.map_err(|_| FAILED)? {
                    match item {
                        tiberius::QueryItem::Metadata(m) => {
                            rows =
                                Rows::new(m.columns().iter().map(|c| c.name().into()).collect())?;
                        }
                        tiberius::QueryItem::Row(row) => {
                            if rows.values.len() >= maximum {
                                break;
                            }
                            rows.push(row.into_iter().map(tds_value).collect::<Result<Vec<_>>>()?)?;
                        }
                    }
                }
                Ok(rows)
            }
            Self::Odbc(c) => match c.execute(sql, (), Some(timeout)).map_err(|_| FAILED)? {
                Some(cursor) => odbc_rows(cursor, maximum),
                None => Ok(Rows::default()),
            },
            Self::Oracle(c) => {
                let mut statement = c
                    .statement(sql)
                    .fetch_array_size(1)
                    .prefetch_rows(1)
                    .lob_locator()
                    .build()
                    .map_err(|_| FAILED)?;
                let result = statement.query(&[]).map_err(|_| FAILED)?;
                let mut rows = Rows::new(
                    result
                        .column_info()
                        .iter()
                        .map(|c| c.name().into())
                        .collect(),
                )?;
                for row in result.take(maximum) {
                    let row = row.map_err(|_| FAILED)?;
                    rows.push(
                        (0..rows.columns.len())
                            .map(|i| oracle_value(&row, i))
                            .collect::<Result<Vec<_>>>()?,
                    )?;
                }
                Ok(rows)
            }
        }
    }
    pub fn access_catalog(&self) -> Result<Rows> {
        if let Self::Odbc(c) = self {
            odbc_rows(
                c.tables("", "", "%", "TABLE,VIEW").map_err(|_| FAILED)?,
                501,
            )
        } else {
            Err(UNSUPPORTED)
        }
    }
    pub fn access_columns(&self, name: &str) -> Result<Rows> {
        if let Self::Odbc(c) = self {
            odbc_rows(c.columns("", "", name, "%").map_err(|_| FAILED)?, 512)
        } else {
            Err(UNSUPPORTED)
        }
    }
}
fn oracle_value(row: &oracle::Row, index: usize) -> Result<Option<String>> {
    use oracle::sql_type::{Blob, Clob, OracleType};
    use std::io::Read;
    match row.column_info()[index].oracle_type() {
        OracleType::CLOB | OracleType::NCLOB => {
            let Some(lob) = row.get::<_, Option<Clob>>(index).map_err(|_| FAILED)? else {
                return Ok(None);
            };
            let mut bytes = Vec::new();
            lob.take(65537)
                .read_to_end(&mut bytes)
                .map_err(|_| FAILED)?;
            if bytes.len() > 65536 {
                return Err(TOO_LARGE);
            }
            Ok(Some(String::from_utf8(bytes).map_err(|_| FAILED)?))
        }
        OracleType::BLOB => {
            let Some(lob) = row.get::<_, Option<Blob>>(index).map_err(|_| FAILED)? else {
                return Ok(None);
            };
            let mut bytes = Vec::new();
            lob.take(32769)
                .read_to_end(&mut bytes)
                .map_err(|_| FAILED)?;
            if bytes.len() > 32768 {
                return Err(TOO_LARGE);
            }
            Ok(Some(format!(
                "hex:{}",
                bytes.iter().map(|v| format!("{v:02x}")).collect::<String>()
            )))
        }
        OracleType::Raw(_) | OracleType::LongRaw => row
            .get::<_, Option<Vec<u8>>>(index)
            .map_err(|_| FAILED)?
            .map(|bytes| {
                if bytes.len() > 32768 {
                    return Err(TOO_LARGE);
                }
                Ok(format!(
                    "hex:{}",
                    bytes.iter().map(|v| format!("{v:02x}")).collect::<String>()
                ))
            })
            .transpose(),
        _ => row.get::<_, Option<String>>(index).map_err(|_| FAILED),
    }
}
fn odbc_rows(mut cursor: impl Cursor, maximum: usize) -> Result<Rows> {
    let mut rows = Rows::new(
        cursor
            .column_names()
            .map_err(|_| FAILED)?
            .map(|c| c.map_err(|_| FAILED))
            .collect::<Result<Vec<_>>>()?,
    )?;
    let binary = (1..=rows.columns.len())
        .map(|i| {
            cursor
                .col_data_type(i as u16)
                .map(|t| {
                    matches!(
                        t,
                        odbc_api::DataType::Binary { .. }
                            | odbc_api::DataType::Varbinary { .. }
                            | odbc_api::DataType::LongVarbinary { .. }
                    )
                })
                .map_err(|_| FAILED)
        })
        .collect::<Result<Vec<_>>>()?;
    while rows.values.len() < maximum {
        let Some(mut row) = cursor.next_row().map_err(|_| FAILED)? else {
            break;
        };
        let mut values = Vec::new();
        for i in 1..=rows.columns.len() {
            if binary[i - 1] {
                let mut buf = odbc_api::parameter::VarBinaryBox::from_buffer(
                    vec![0u8; 32768].into_boxed_slice(),
                    odbc_api::buffers::Indicator::Null,
                );
                row.get_data(i as u16, &mut buf).map_err(|_| FAILED)?;
                if !buf.is_complete() {
                    return Err(TOO_LARGE);
                }
                values.push(buf.as_slice().map(|bytes| {
                    format!(
                        "hex:{}",
                        bytes.iter().map(|v| format!("{v:02x}")).collect::<String>()
                    )
                }));
                continue;
            }
            let mut buf = odbc_api::parameter::VarWCharBox::from_buffer(
                vec![0u16; 65537].into_boxed_slice(),
                odbc_api::buffers::Indicator::Null,
            );
            row.get_data(i as u16, &mut buf).map_err(|_| FAILED)?;
            if !buf.is_complete() {
                return Err(TOO_LARGE);
            }
            values.push(
                buf.as_slice()
                    .map(String::from_utf16)
                    .transpose()
                    .map_err(|_| FAILED)?,
            );
        }
        rows.push(values)?;
    }
    Ok(rows)
}
fn tds_value(value: tiberius::ColumnData<'static>) -> Result<Option<String>> {
    use tiberius::ColumnData::*;
    let text = match value {
        U8(v) => v.map(|v| v.to_string()),
        I16(v) => v.map(|v| v.to_string()),
        I32(v) => v.map(|v| v.to_string()),
        I64(v) => v.map(|v| v.to_string()),
        F32(v) => v.map(|v| v.to_string()),
        F64(v) => v.map(|v| v.to_string()),
        Bit(v) => v.map(|v| if v { "1" } else { "0" }.into()),
        String(v) => v.map(|v| v.into_owned()),
        Guid(v) => v.map(|v| v.to_string()),
        Numeric(v) => v.map(|v| v.to_string()),
        Xml(v) => v.map(|v| v.to_string()),
        Binary(v) => {
            if v.as_ref().is_some_and(|b| b.len() > 32768) {
                return Err(TOO_LARGE);
            }
            v.map(|v| {
                format!(
                    "hex:{}",
                    v.iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<std::string::String>()
                )
            })
        }
        v @ (DateTime(_) | SmallDateTime(_) | DateTime2(_)) => {
            <chrono::NaiveDateTime as tiberius::FromSql>::from_sql(&v)
                .map_err(|_| FAILED)?
                .map(|v| v.to_string())
        }
        v @ Time(_) => <chrono::NaiveTime as tiberius::FromSql>::from_sql(&v)
            .map_err(|_| FAILED)?
            .map(|v| v.to_string()),
        v @ Date(_) => <chrono::NaiveDate as tiberius::FromSql>::from_sql(&v)
            .map_err(|_| FAILED)?
            .map(|v| v.to_string()),
        v @ DateTimeOffset(_) => {
            <chrono::DateTime<chrono::FixedOffset> as tiberius::FromSql>::from_sql(&v)
                .map_err(|_| FAILED)?
                .map(|v| v.to_rfc3339())
        }
    };
    if text.as_ref().is_some_and(|s| s.len() > 65536) {
        return Err(TOO_LARGE);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oracle_tls_is_explicit_and_descriptor_cannot_be_injected() {
        let mut p = serde_json::json!({"host":"db.example","database":"service","port":1521,"tls":"VerifyIdentity"});
        let address = oracle_address(&p, 5).unwrap();
        assert!(
            address.contains("PROTOCOL=TCPS")
                && address.contains("SSL_SERVER_DN_MATCH=YES")
                && address.contains("WALLET_LOCATION=SYSTEM")
        );
        p["tls"] = serde_json::json!("Required");
        assert!(oracle_address(&p, 5).is_err());
        p["tls"] = serde_json::json!("Disabled");
        assert!(oracle_address(&p, 5).unwrap().contains("PROTOCOL=TCP)"));
        p["host"] = serde_json::json!("host)(PORT=1");
        assert!(oracle_address(&p, 5).is_err());
    }
    #[test]
    fn tds_values_preserve_null_unicode_binary_and_numbers() {
        use tiberius::ColumnData;
        assert_eq!(
            tds_value(ColumnData::I64(Some(i64::MAX))).unwrap(),
            Some(i64::MAX.to_string())
        );
        assert_eq!(
            tds_value(ColumnData::String(Some("字段🙂".into()))).unwrap(),
            Some("字段🙂".into())
        );
        assert_eq!(tds_value(ColumnData::String(None)).unwrap(), None);
        assert_eq!(
            tds_value(ColumnData::Binary(Some(vec![0, 255].into()))).unwrap(),
            Some("hex:00ff".into())
        );
        assert!(tds_value(ColumnData::Binary(Some(vec![0; 32769].into()))).is_err());
    }
    #[test]
    fn odbc_values_cannot_inject_attributes() {
        assert_eq!(odbc_value("x};UID=other").unwrap(), "{x}};UID=other}");
        assert!(odbc_value("bad\0value").is_err());
    }
}
