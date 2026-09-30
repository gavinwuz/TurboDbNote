//! Curated common syntax, not a claim of server-version or extension availability.
use crate::DatabaseKind;
const COMMON: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "AS",
    "DISTINCT",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "ON",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "ASC",
    "DESC",
    "IS NULL",
    "IS NOT NULL",
    "IN",
    "BETWEEN",
    "LIKE",
    "EXISTS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "UNION ALL",
    "INSERT INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "CREATE TABLE",
    "ALTER TABLE",
    "DROP TABLE",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "WITH",
];
pub fn keywords(kind: Option<DatabaseKind>) -> Vec<&'static str> {
    let extra: &[&str] = match kind {
        Some(DatabaseKind::MySql) => &[
            "SHOW TABLES",
            "SHOW DATABASES",
            "DESCRIBE",
            "EXPLAIN",
            "ON DUPLICATE KEY UPDATE",
            "AUTO_INCREMENT",
            "ENGINE",
            "TRUNCATE TABLE",
        ],
        Some(DatabaseKind::PostgreSql) => &[
            "ILIKE",
            "RETURNING",
            "ON CONFLICT",
            "DO NOTHING",
            "DISTINCT ON",
            "EXPLAIN ANALYZE",
            "TRUNCATE TABLE",
        ],
        Some(DatabaseKind::Sqlite) => &[
            "PRAGMA",
            "INSERT OR REPLACE",
            "INSERT OR IGNORE",
            "VACUUM",
            "EXPLAIN QUERY PLAN",
            "ON CONFLICT",
        ],
        Some(DatabaseKind::ClickHouse) => &[
            "PREWHERE",
            "FINAL",
            "FORMAT",
            "ARRAY JOIN",
            "LIMIT BY",
            "SAMPLE",
            "SETTINGS",
            "SHOW TABLES",
            "DESCRIBE TABLE",
            "TRUNCATE TABLE",
        ],
        _ => &[],
    };
    COMMON
        .iter()
        .chain(extra)
        .copied()
        .filter(|word| {
            kind != Some(DatabaseKind::ClickHouse)
                || !matches!(*word, "BEGIN" | "COMMIT" | "ROLLBACK" | "UPDATE")
        })
        .collect()
}
const FUNCTIONS: &[(&str, &str)] = &[
    ("COUNT", "COUNT(*) / COUNT(expression)"),
    ("SUM", "SUM(expression)"),
    ("AVG", "AVG(expression)"),
    ("MIN", "MIN(expression)"),
    ("MAX", "MAX(expression)"),
    ("COALESCE", "COALESCE(value, fallback, ...)"),
    ("NULLIF", "NULLIF(value, other)"),
    ("LOWER", "LOWER(text)"),
    ("UPPER", "UPPER(text)"),
    ("LENGTH", "LENGTH(value)"),
    ("ROUND", "ROUND(number, digits)"),
    ("ABS", "ABS(number)"),
    ("TRIM", "TRIM(text)"),
];
pub fn functions(kind: Option<DatabaseKind>) -> Vec<(&'static str, &'static str)> {
    let extra: &[(&str, &str)] = match kind {
        Some(DatabaseKind::MySql) => &[
            ("IFNULL", "IFNULL(value, fallback)"),
            ("IF", "IF(condition, yes, no)"),
            ("CONCAT", "CONCAT(text, ...)"),
            ("CONCAT_WS", "CONCAT_WS(separator, text, ...)"),
            ("SUBSTRING", "SUBSTRING(text, position, length)"),
            ("DATE_FORMAT", "DATE_FORMAT(date, '%Y-%m-%d')"),
            ("NOW", "NOW()"),
            ("CURDATE", "CURDATE()"),
            ("DATEDIFF", "DATEDIFF(date1, date2)"),
            ("DATE_ADD", "DATE_ADD(date, INTERVAL value DAY)"),
            ("GROUP_CONCAT", "GROUP_CONCAT(expression SEPARATOR ',')"),
            (
                "JSON_EXTRACT",
                "JSON_EXTRACT(document, '$.path') — MySQL 5.7+",
            ),
            (
                "ROW_NUMBER",
                "ROW_NUMBER() OVER (ORDER BY column) — MySQL 8.0+",
            ),
        ],
        Some(DatabaseKind::PostgreSql) => &[
            ("STRING_AGG", "STRING_AGG(expression, delimiter)"),
            ("DATE_TRUNC", "DATE_TRUNC('day', timestamp)"),
            ("TO_CHAR", "TO_CHAR(value, format)"),
            ("NOW", "NOW()"),
            ("CONCAT", "CONCAT(value, ...)"),
            ("SUBSTRING", "SUBSTRING(text FROM start FOR count)"),
            ("EXTRACT", "EXTRACT(YEAR FROM timestamp)"),
            (
                "JSONB_EXTRACT_PATH_TEXT",
                "JSONB_EXTRACT_PATH_TEXT(document, path, ...)",
            ),
        ],
        Some(DatabaseKind::Sqlite) => &[
            ("IFNULL", "IFNULL(value, fallback)"),
            ("SUBSTR", "SUBSTR(text, start, length)"),
            ("INSTR", "INSTR(text, substring)"),
            ("PRINTF", "PRINTF(format, ...)"),
            ("DATE", "DATE(time_value, modifier, ...)"),
            ("DATETIME", "DATETIME(time_value, modifier, ...)"),
            ("STRFTIME", "STRFTIME('%Y-%m-%d', time_value)"),
            ("JULIANDAY", "JULIANDAY(time_value)"),
            ("GROUP_CONCAT", "GROUP_CONCAT(expression, separator)"),
            (
                "JSON_EXTRACT",
                "JSON_EXTRACT(document, '$.path') — JSON support required",
            ),
        ],
        Some(DatabaseKind::ClickHouse) => &[
            ("toDate", "toDate(value)"),
            ("toDateTime", "toDateTime(value[, timezone])"),
            ("toStartOfDay", "toStartOfDay(datetime)"),
            (
                "formatDateTime",
                "formatDateTime(datetime, format[, timezone])",
            ),
            ("uniqExact", "uniqExact(expression)"),
            ("countIf", "countIf(condition)"),
            ("sumIf", "sumIf(expression, condition)"),
            ("ifNull", "ifNull(value, fallback)"),
            ("multiIf", "multiIf(condition, then, ..., else)"),
            ("arrayJoin", "arrayJoin(array)"),
            ("JSONExtractString", "JSONExtractString(json, key, ...)"),
        ],
        _ => &[],
    };
    if kind == Some(DatabaseKind::Redis) {
        return vec![];
    }
    if kind == Some(DatabaseKind::ClickHouse) {
        return [
            ("count", "count(*) / count(expression)"),
            ("sum", "sum(expression)"),
            ("avg", "avg(expression)"),
            ("min", "min(expression)"),
            ("max", "max(expression)"),
            ("coalesce", "coalesce(value, fallback, ...)"),
            ("lower", "lower(text)"),
            ("upper", "upper(text)"),
            ("length", "length(value)"),
            ("round", "round(number, digits)"),
            ("abs", "abs(number)"),
        ]
        .into_iter()
        .chain(extra.iter().copied())
        .collect();
    }
    FUNCTIONS.iter().chain(extra).copied().collect()
}
pub const REDIS: &[(&str, &str)] = &[
    ("GET", "GET key"),
    ("SET", "SET key value [EX seconds] [NX|XX]"),
    ("MGET", "MGET key [key ...]"),
    ("DEL", "DEL key [key ...]"),
    ("UNLINK", "UNLINK key [key ...]"),
    ("EXISTS", "EXISTS key [key ...]"),
    ("EXPIRE", "EXPIRE key seconds"),
    ("TTL", "TTL key"),
    ("TYPE", "TYPE key"),
    ("SCAN", "SCAN cursor [MATCH pattern] [COUNT count]"),
    ("HGET", "HGET key field"),
    ("HGETALL", "HGETALL key"),
    ("HSET", "HSET key field value [field value ...]"),
    ("LPUSH", "LPUSH key element [element ...]"),
    ("LRANGE", "LRANGE key start stop"),
    ("SADD", "SADD key member [member ...]"),
    ("SMEMBERS", "SMEMBERS key"),
    ("ZADD", "ZADD key score member [score member ...]"),
    ("ZRANGE", "ZRANGE key start stop [WITHSCORES]"),
    ("PING", "PING [message]"),
    ("INFO", "INFO [section]"),
    ("DBSIZE", "DBSIZE"),
    ("SELECT", "SELECT index (standalone only)"),
];
