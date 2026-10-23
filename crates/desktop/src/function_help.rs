//! Offline function help. Templates are editable examples, never execution requests.
use turbodbn_services::DatabaseKind;
pub struct Help {
    pub summary: &'static str,
    pub parameters: &'static str,
    pub call: &'static str,
    pub example: &'static str,
    pub more: Option<&'static str>,
}
pub fn lookup(kind: Option<DatabaseKind>, name: &str) -> Option<Help> {
    if kind == Some(DatabaseKind::Redis) {
        return None;
    }
    match name.to_ascii_uppercase().as_str() {
        "COUNT" => Some(Help {
            summary: "sql_function.count.summary",
            parameters: "sql_function.count.parameters",
            call: "COUNT(expression)",
            example: "SELECT COUNT(*);",
            more: None,
        }),
        "SUM" => Some(Help {
            summary: "sql_function.sum.summary",
            parameters: "sql_function.sum.parameters",
            call: "SUM(expression)",
            example: "SELECT SUM(10);",
            more: None,
        }),
        "AVG" => Some(Help {
            summary: "sql_function.avg.summary",
            parameters: "sql_function.avg.parameters",
            call: "AVG(expression)",
            example: "SELECT AVG(10);",
            more: None,
        }),
        "MIN" => Some(Help {
            summary: "sql_function.min.summary",
            parameters: "sql_function.min.parameters",
            call: "MIN(expression)",
            example: "SELECT MIN(10);",
            more: None,
        }),
        "MAX" => Some(Help {
            summary: "sql_function.max.summary",
            parameters: "sql_function.max.parameters",
            call: "MAX(expression)",
            example: "SELECT MAX(10);",
            more: None,
        }),
        "COALESCE" => Some(Help {
            summary: "sql_function.coalesce.summary",
            parameters: "sql_function.coalesce.parameters",
            call: "COALESCE(value, fallback)",
            example: "SELECT COALESCE(NULL, 'fallback');",
            more: None,
        }),
        "NULLIF" => Some(Help {
            summary: "sql_function.nullif.summary",
            parameters: "sql_function.nullif.parameters",
            call: "NULLIF(value, other)",
            example: "SELECT NULLIF(10, 10);",
            more: None,
        }),
        "LOWER" => Some(Help {
            summary: "sql_function.lower.summary",
            parameters: "sql_function.lower.parameters",
            call: "LOWER(text)",
            example: "SELECT LOWER('ABC');",
            more: None,
        }),
        "UPPER" => Some(Help {
            summary: "sql_function.upper.summary",
            parameters: "sql_function.upper.parameters",
            call: "UPPER(text)",
            example: "SELECT UPPER('abc');",
            more: None,
        }),
        "LENGTH" => Some(Help {
            summary: "sql_function.length.summary",
            parameters: "sql_function.length.parameters",
            call: "LENGTH(value)",
            example: "SELECT LENGTH('hello');",
            more: None,
        }),
        "ROUND" => Some(Help {
            summary: "sql_function.round.summary",
            parameters: "sql_function.round.parameters",
            call: "ROUND(number, digits)",
            example: "SELECT ROUND(12.345, 2);",
            more: Some("SELECT ROUND(123.456, 1);"),
        }),
        "ABS" => Some(Help {
            summary: "sql_function.abs.summary",
            parameters: "sql_function.abs.parameters",
            call: "ABS(number)",
            example: "SELECT ABS(-12);",
            more: None,
        }),
        "TRIM" => Some(Help {
            summary: "sql_function.trim.summary",
            parameters: "sql_function.trim.parameters",
            call: "TRIM(text)",
            example: "SELECT TRIM('  hello  ');",
            more: None,
        }),
        "IFNULL" if matches!(kind, Some(DatabaseKind::MySql | DatabaseKind::Sqlite)) => {
            Some(Help {
                summary: "sql_function.ifnull.summary",
                parameters: "sql_function.ifnull.parameters",
                call: "IFNULL(value, fallback)",
                example: "SELECT IFNULL(NULL, 'unknown');",
                more: None,
            })
        }
        "IF" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.if.summary",
            parameters: "sql_function.if.parameters",
            call: "IF(condition, yes, no)",
            example: "SELECT IF(1 > 0, 'yes', 'no');",
            more: None,
        }),
        "CONCAT" if matches!(kind, Some(DatabaseKind::MySql | DatabaseKind::PostgreSql)) => {
            Some(Help {
                summary: "sql_function.concat.summary",
                parameters: "sql_function.concat.parameters",
                call: "CONCAT(text, other)",
                example: "SELECT CONCAT('hello', ' world');",
                more: None,
            })
        }
        "CONCAT_WS" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.concat_ws.summary",
            parameters: "sql_function.concat_ws.parameters",
            call: "CONCAT_WS(separator, text, other)",
            example: "SELECT CONCAT_WS('-', '2026', '10', '22');",
            more: None,
        }),
        "SUBSTRING" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.substring_mysql.summary",
            parameters: "sql_function.substring_mysql.parameters",
            call: "SUBSTRING(text, position, length)",
            example: "SELECT SUBSTRING('abcdef', 2, 3);",
            more: None,
        }),
        "DATE_FORMAT" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.date_format.summary",
            parameters: "sql_function.date_format.parameters",
            call: "DATE_FORMAT(date, format)",
            example: "SELECT DATE_FORMAT('2026-10-22 14:30:00', '%Y-%m-%d');",
            more: Some("SELECT DATE_FORMAT('2026-10-22 14:30:00', '%H:%i:%s');"),
        }),
        "NOW" if matches!(kind, Some(DatabaseKind::MySql | DatabaseKind::PostgreSql)) => {
            Some(Help {
                summary: "sql_function.now.summary",
                parameters: "sql_function.now.parameters",
                call: "NOW()",
                example: "SELECT NOW();",
                more: None,
            })
        }
        "CURDATE" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.curdate.summary",
            parameters: "sql_function.curdate.parameters",
            call: "CURDATE()",
            example: "SELECT CURDATE();",
            more: None,
        }),
        "DATEDIFF" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.datediff.summary",
            parameters: "sql_function.datediff.parameters",
            call: "DATEDIFF(date1, date2)",
            example: "SELECT DATEDIFF('2026-10-22', '2026-10-20');",
            more: None,
        }),
        "DATE_ADD" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.date_add.summary",
            parameters: "sql_function.date_add.parameters",
            call: "DATE_ADD(date, INTERVAL value DAY)",
            example: "SELECT DATE_ADD('2026-10-22', INTERVAL 7 DAY);",
            more: None,
        }),
        "GROUP_CONCAT" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.group_concat_mysql.summary",
            parameters: "sql_function.group_concat_mysql.parameters",
            call: "GROUP_CONCAT(expression SEPARATOR ',')",
            example: "SELECT GROUP_CONCAT('hello' SEPARATOR ',');",
            more: None,
        }),
        "STRING_AGG" if kind == Some(DatabaseKind::PostgreSql) => Some(Help {
            summary: "sql_function.string_agg.summary",
            parameters: "sql_function.string_agg.parameters",
            call: "STRING_AGG(expression, delimiter)",
            example: "SELECT STRING_AGG(value, ',') FROM (VALUES ('a'), ('b')) AS sample(value);",
            more: None,
        }),
        "DATE_TRUNC" if kind == Some(DatabaseKind::PostgreSql) => Some(Help {
            summary: "sql_function.date_trunc.summary",
            parameters: "sql_function.date_trunc.parameters",
            call: "DATE_TRUNC(precision, timestamp)",
            example: "SELECT DATE_TRUNC('day', TIMESTAMP '2026-10-22 14:30:00');",
            more: None,
        }),
        "TO_CHAR" if kind == Some(DatabaseKind::PostgreSql) => Some(Help {
            summary: "sql_function.to_char.summary",
            parameters: "sql_function.to_char.parameters",
            call: "TO_CHAR(value, format)",
            example: "SELECT TO_CHAR(TIMESTAMP '2026-10-22 14:30:00', 'YYYY-MM-DD');",
            more: None,
        }),
        "EXTRACT" if kind == Some(DatabaseKind::PostgreSql) => Some(Help {
            summary: "sql_function.extract.summary",
            parameters: "sql_function.extract.parameters",
            call: "EXTRACT(YEAR FROM timestamp)",
            example: "SELECT EXTRACT(YEAR FROM TIMESTAMP '2026-10-22 14:30:00');",
            more: None,
        }),
        "SUBSTRING" if kind == Some(DatabaseKind::PostgreSql) => Some(Help {
            summary: "sql_function.substring_pg.summary",
            parameters: "sql_function.substring_pg.parameters",
            call: "SUBSTRING(text FROM start FOR count)",
            example: "SELECT SUBSTRING('abcdef' FROM 2 FOR 3);",
            more: None,
        }),
        "SUBSTR" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.substr.summary",
            parameters: "sql_function.substr.parameters",
            call: "SUBSTR(text, start, length)",
            example: "SELECT SUBSTR('abcdef', 2, 3);",
            more: None,
        }),
        "INSTR" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.instr.summary",
            parameters: "sql_function.instr.parameters",
            call: "INSTR(text, substring)",
            example: "SELECT INSTR('hello', 'll');",
            more: None,
        }),
        "DATE" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.date.summary",
            parameters: "sql_function.date.parameters",
            call: "DATE(time_value)",
            example: "SELECT DATE('2026-10-22', '+7 days');",
            more: None,
        }),
        "DATETIME" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.datetime.summary",
            parameters: "sql_function.datetime.parameters",
            call: "DATETIME(time_value)",
            example: "SELECT DATETIME('2026-10-22 14:30:00', '+1 hour');",
            more: None,
        }),
        "STRFTIME" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.strftime.summary",
            parameters: "sql_function.strftime.parameters",
            call: "STRFTIME(format, time_value)",
            example: "SELECT STRFTIME('%Y-%m-%d', '2026-10-22 14:30:00');",
            more: Some("SELECT STRFTIME('%H:%M:%S', '2026-10-22 14:30:00');"),
        }),
        "JULIANDAY" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.julianday.summary",
            parameters: "sql_function.julianday.parameters",
            call: "JULIANDAY(time_value)",
            example: "SELECT JULIANDAY('2026-10-22');",
            more: None,
        }),
        "PRINTF" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.printf.summary",
            parameters: "sql_function.printf.parameters",
            call: "PRINTF(format, value)",
            example: "SELECT PRINTF('%.2f', 12.345);",
            more: None,
        }),
        "JSON_EXTRACT" if matches!(kind, Some(DatabaseKind::MySql | DatabaseKind::Sqlite)) => {
            Some(Help {
                summary: "sql_function.json_extract.summary",
                parameters: "sql_function.json_extract.parameters",
                call: "JSON_EXTRACT(document, path)",
                example: "SELECT JSON_EXTRACT('{\"name\":\"Ada\"}', '$.name');",
                more: None,
            })
        }
        "ROW_NUMBER" if kind == Some(DatabaseKind::MySql) => Some(Help {
            summary: "sql_function.row_number.summary",
            parameters: "sql_function.row_number.parameters",
            call: "ROW_NUMBER() OVER (ORDER BY column)",
            example: "SELECT ROW_NUMBER() OVER (ORDER BY value) FROM (SELECT 10 AS value UNION ALL SELECT 20) AS sample;",
            more: None,
        }),
        "JSONB_EXTRACT_PATH_TEXT" if kind == Some(DatabaseKind::PostgreSql) => Some(Help {
            summary: "sql_function.jsonb_extract_path_text.summary",
            parameters: "sql_function.jsonb_extract_path_text.parameters",
            call: "JSONB_EXTRACT_PATH_TEXT(document, path)",
            example: "SELECT JSONB_EXTRACT_PATH_TEXT('{\"name\":\"Ada\"}'::jsonb, 'name');",
            more: None,
        }),
        "GROUP_CONCAT" if kind == Some(DatabaseKind::Sqlite) => Some(Help {
            summary: "sql_function.group_concat_sqlite.summary",
            parameters: "sql_function.group_concat_sqlite.parameters",
            call: "GROUP_CONCAT(expression, separator)",
            example: "SELECT GROUP_CONCAT('hello', ',');",
            more: None,
        }),
        "TODATE" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.todate.summary",
            parameters: "sql_function.todate.parameters",
            call: "toDate(value)",
            example: "SELECT toDate('2026-10-22');",
            more: None,
        }),
        "TODATETIME" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.todatetime.summary",
            parameters: "sql_function.todatetime.parameters",
            call: "toDateTime(value)",
            example: "SELECT toDateTime('2026-10-22 14:30:00');",
            more: None,
        }),
        "TOSTARTOFDAY" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.tostartofday.summary",
            parameters: "sql_function.tostartofday.parameters",
            call: "toStartOfDay(datetime)",
            example: "SELECT toStartOfDay(toDateTime('2026-10-22 14:30:00'));",
            more: None,
        }),
        "FORMATDATETIME" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.formatdatetime.summary",
            parameters: "sql_function.formatdatetime.parameters",
            call: "formatDateTime(datetime, format)",
            example: "SELECT formatDateTime(toDateTime('2026-10-22 14:30:00'), '%F');",
            more: None,
        }),
        "UNIQEXACT" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.uniqexact.summary",
            parameters: "sql_function.uniqexact.parameters",
            call: "uniqExact(expression)",
            example: "SELECT uniqExact(10);",
            more: None,
        }),
        "COUNTIF" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.countif.summary",
            parameters: "sql_function.countif.parameters",
            call: "countIf(condition)",
            example: "SELECT countIf(1 > 0);",
            more: None,
        }),
        "SUMIF" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.sumif.summary",
            parameters: "sql_function.sumif.parameters",
            call: "sumIf(expression, condition)",
            example: "SELECT sumIf(10, 1 > 0);",
            more: None,
        }),
        "IFNULL" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.ifnull_ch.summary",
            parameters: "sql_function.ifnull_ch.parameters",
            call: "ifNull(value, fallback)",
            example: "SELECT ifNull(NULL, 'unknown');",
            more: None,
        }),
        "MULTIIF" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.multiif.summary",
            parameters: "sql_function.multiif.parameters",
            call: "multiIf(condition, yes, no)",
            example: "SELECT multiIf(1 > 0, 'yes', 'no');",
            more: None,
        }),
        "ARRAYJOIN" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.arrayjoin.summary",
            parameters: "sql_function.arrayjoin.parameters",
            call: "arrayJoin(array)",
            example: "SELECT arrayJoin([1, 2, 3]);",
            more: None,
        }),
        "JSONEXTRACTSTRING" if kind == Some(DatabaseKind::ClickHouse) => Some(Help {
            summary: "sql_function.jsonextractstring.summary",
            parameters: "sql_function.jsonextractstring.parameters",
            call: "JSONExtractString(json, key)",
            example: "SELECT JSONExtractString('{\"name\":\"Ada\"}', 'name');",
            more: None,
        }),
        _ => None,
    }
}

/// Keep an existing argument list intact; otherwise return the first editable placeholder.
pub fn insertion(
    kind: Option<DatabaseKind>,
    name: &str,
    tail: &str,
) -> (String, Option<std::ops::Range<usize>>) {
    let dialect = kind
        .and_then(turbodbn_services::sql_script::dialect)
        .unwrap_or(turbodbn_core::notebook::SqlDialect::PostgreSql);
    let first = turbodbn_services::sql_script::leading_code_offset(tail, dialect);
    if tail[first..].starts_with('(') {
        return (name.into(), None);
    }
    let call = lookup(kind, name)
        .map(|h| {
            h.call
                .replacen(h.call.split('(').next().unwrap_or_default(), name, 1)
        })
        .unwrap_or_else(|| format!("{name}()"));
    let parameter = call.find('(').map(|i| {
        let start = i + 1;
        let end = call[start..]
            .find([',', ')', ' '])
            .map_or(start, |i| start + i);
        start..end
    });
    let parameter = if name.eq_ignore_ascii_case("ROW_NUMBER") {
        call.find("column").map(|i| i..i + 6)
    } else {
        parameter
    };
    (call, parameter)
}
pub fn keyword_summary(name: &str) -> Option<&'static str> {
    match name {
        "SELECT" => Some("sql_keyword.select"),
        "FROM" => Some("sql_keyword.from"),
        "WHERE" => Some("sql_keyword.where"),
        "DISTINCT" => Some("sql_keyword.distinct"),
        "DESC" => Some("sql_keyword.desc"),
        "ASC" => Some("sql_keyword.asc"),
        "ORDER BY" => Some("sql_keyword.order_by"),
        "GROUP BY" => Some("sql_keyword.group_by"),
        "HAVING" => Some("sql_keyword.having"),
        "JOIN" => Some("sql_keyword.join"),
        "LEFT JOIN" => Some("sql_keyword.left_join"),
        "INNER JOIN" => Some("sql_keyword.inner_join"),
        "ON" => Some("sql_keyword.on"),
        "LIMIT" => Some("sql_keyword.limit"),
        "OFFSET" => Some("sql_keyword.offset"),
        "AS" => Some("sql_keyword.as"),
        "AND" => Some("sql_keyword.and"),
        "OR" => Some("sql_keyword.or"),
        "NOT" => Some("sql_keyword.not"),
        "CASE" => Some("sql_keyword.case"),
        "WHEN" => Some("sql_keyword.when"),
        "THEN" => Some("sql_keyword.then"),
        "ELSE" => Some("sql_keyword.else"),
        "END" => Some("sql_keyword.end"),
        "WITH" => Some("sql_keyword.with"),
        "INSERT INTO" => Some("sql_keyword.insert_into"),
        "UPDATE" => Some("sql_keyword.update"),
        "DELETE FROM" => Some("sql_keyword.delete_from"),
        "DROP TABLE" => Some("sql_keyword.drop_table"),
        "CREATE TABLE" => Some("sql_keyword.create_table"),
        "ALTER TABLE" => Some("sql_keyword.alter_table"),
        "DESCRIBE" => Some("sql_keyword.describe"),
        "SHOW TABLES" => Some("sql_keyword.show_tables"),
        "SHOW DATABASES" => Some("sql_keyword.show_databases"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{insertion, lookup};
    use turbodbn_services::DatabaseKind;
    #[test]
    fn built_in_catalogue_functions_have_offline_help() {
        use turbodbn_services::database::sql_assist::{SuggestionKind, complete};
        for kind in [
            DatabaseKind::MySql,
            DatabaseKind::PostgreSql,
            DatabaseKind::Sqlite,
            DatabaseKind::ClickHouse,
        ] {
            for prefix in 'A'..='Z' {
                let source = format!("SELECT {prefix}");
                for item in complete(Some(kind), &source, source.len(), &[]) {
                    if item.kind == SuggestionKind::Function {
                        assert!(
                            lookup(Some(kind), &item.label).is_some(),
                            "{kind:?}: {}",
                            item.label
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn dialect_templates_preserve_existing_arguments_and_function_case() {
        assert!(lookup(Some(DatabaseKind::Sqlite), "DATE_FORMAT").is_none());
        assert!(lookup(Some(DatabaseKind::MySql), "STRFTIME").is_none());
        let (call, range) = insertion(Some(DatabaseKind::MySql), "DATE_FORMAT", "");
        assert_eq!(call, "DATE_FORMAT(date, format)");
        assert_eq!(&call[range.unwrap()], "date");
        assert_eq!(
            insertion(Some(DatabaseKind::MySql), "UPPER", " /* keep */ ('abc')"),
            ("UPPER".into(), None)
        );
        assert_eq!(
            insertion(
                Some(DatabaseKind::MySql),
                "DATE_FORMAT",
                "  (created_at, '%Y')"
            ),
            ("DATE_FORMAT".into(), None)
        );
        let (call, range) = insertion(Some(DatabaseKind::ClickHouse), "upper", "");
        assert_eq!(call, "upper(text)");
        assert_eq!(&call[range.unwrap()], "text");
        assert_eq!(insertion(Some(DatabaseKind::MySql), "NOW", "").0, "NOW()");
        assert_eq!(
            insertion(Some(DatabaseKind::PostgreSql), "SUBSTRING", "").0,
            "SUBSTRING(text FROM start FOR count)"
        );
        assert!(lookup(Some(DatabaseKind::Redis), "COUNT").is_none());
    }
}
