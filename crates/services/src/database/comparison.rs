//! Bounded comparison; results describe samples, never whole-table equivalence.
use super::workbench::{Rows, Schema};
use std::collections::{BTreeMap, BTreeSet};
pub struct Compared {
    pub left: Rows,
    pub right: Rows,
    pub different: usize,
    pub keyed: bool,
}
pub fn data(left: &Rows, ls: &Schema, right: &Rows, rs: &Schema) -> Compared {
    let lk: Vec<_> = ls
        .columns
        .iter()
        .filter(|c| c.primary)
        .map(|c| c.name.as_str())
        .collect();
    let rk: Vec<_> = rs
        .columns
        .iter()
        .filter(|c| c.primary)
        .map(|c| c.name.as_str())
        .collect();
    let indices = |r: &Rows, keys: &[&str]| {
        keys.iter()
            .map(|k| r.columns.iter().position(|c| c == k))
            .collect::<Option<Vec<_>>>()
    };
    let index = |r: &Rows, keys: &[usize]| -> Option<BTreeMap<String, usize>> {
        let mut map = BTreeMap::new();
        for (i, row) in r.values.iter().enumerate() {
            let key = keys
                .iter()
                .map(|k| row.get(*k)?.as_ref())
                .collect::<Option<Vec<_>>>()?;
            if map.insert(serde_json::to_string(&key).ok()?, i).is_some() {
                return None;
            }
        }
        Some(map)
    };
    let compatible = lk.iter().all(|name| {
        ls.columns
            .iter()
            .find(|c| c.name == *name)
            .zip(rs.columns.iter().find(|c| c.name == *name))
            .is_some_and(|(l, r)| l.data_type.eq_ignore_ascii_case(&r.data_type))
    });
    let maps = (!lk.is_empty() && lk == rk && compatible)
        .then(|| {
            Some((
                index(left, &indices(left, &lk)?)?,
                index(right, &indices(right, &rk)?)?,
            ))
        })
        .flatten();
    let keyed = maps.is_some();
    let pairs: Vec<_> = if let Some((l, r)) = maps {
        l.keys()
            .chain(r.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|k| (l.get(k).copied(), r.get(k).copied()))
            .collect()
    } else {
        (0..left.values.len().max(right.values.len()))
            .map(|i| {
                (
                    (i < left.values.len()).then_some(i),
                    (i < right.values.len()).then_some(i),
                )
            })
            .collect()
    };
    let mut output = Compared {
        left: Rows {
            columns: std::iter::once("Δ".into())
                .chain(left.columns.clone())
                .collect(),
            values: vec![],
        },
        right: Rows {
            columns: std::iter::once("Δ".into())
                .chain(right.columns.clone())
                .collect(),
            values: vec![],
        },
        different: 0,
        keyed,
    };
    let right_columns = left
        .columns
        .iter()
        .map(|c| right.columns.iter().position(|v| v == c))
        .collect::<Option<Vec<_>>>();
    for (l, r) in pairs {
        let same = match (l, r, &right_columns) {
            (Some(l), Some(r), Some(cols)) if left.columns.len() == right.columns.len() => cols
                .iter()
                .enumerate()
                .all(|(i, j)| left.values[l].get(i) == right.values[r].get(*j)),
            _ => false,
        };
        if !same {
            output.different += 1;
        }
        for (source, pos, dest) in [(left, l, &mut output.left), (right, r, &mut output.right)] {
            let marker = if pos.is_none() {
                "∅"
            } else if same {
                "="
            } else {
                "≠"
            };
            let mut row = vec![Some(marker.into())];
            row.extend(
                pos.map(|i| source.values[i].clone())
                    .unwrap_or_else(|| vec![None; source.columns.len()]),
            );
            dest.values.push(row);
        }
    }
    output
}
pub fn ddl(left: &str, right: &str) -> Result<Compared, String> {
    let l: Vec<_> = left.lines().collect();
    let r: Vec<_> = right.lines().collect();
    if l.len() > 2000 || r.len() > 2000 {
        return Err("compare.ddl_limit".into());
    }
    let n = r.len() + 1;
    let mut dp = vec![0u16; (l.len() + 1) * n];
    for i in (0..l.len()).rev() {
        for j in (0..r.len()).rev() {
            dp[i * n + j] = if l[i] == r[j] {
                1 + dp[(i + 1) * n + j + 1]
            } else {
                dp[(i + 1) * n + j].max(dp[i * n + j + 1])
            };
        }
    }
    let mut out = Compared {
        left: Rows {
            columns: vec!["Δ".into(), "DDL".into()],
            values: vec![],
        },
        right: Rows {
            columns: vec!["Δ".into(), "DDL".into()],
            values: vec![],
        },
        different: 0,
        keyed: false,
    };
    let (mut i, mut j) = (0, 0);
    while i < l.len() || j < r.len() {
        let (a, b, same) = if i < l.len() && j < r.len() && l[i] == r[j] {
            let p = (Some(l[i]), Some(r[j]), true);
            i += 1;
            j += 1;
            p
        } else if i < l.len() && (j == r.len() || dp[(i + 1) * n + j] >= dp[i * n + j + 1]) {
            let p = (Some(l[i]), None, false);
            i += 1;
            p
        } else {
            let p = (None, Some(r[j]), false);
            j += 1;
            p
        };
        if !same {
            out.different += 1;
        }
        for (text, rows, marker) in [(a, &mut out.left, "-"), (b, &mut out.right, "+")] {
            rows.values.push(vec![
                Some(
                    if same {
                        "="
                    } else if text.is_none() {
                        "∅"
                    } else {
                        marker
                    }
                    .into(),
                ),
                text.map(str::to_owned),
            ]);
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::super::workbench::Column;
    use super::*;
    #[test]
    fn key_alignment_and_nulls() {
        let s = Schema {
            columns: vec![Column {
                name: "id".into(),
                primary: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let rows = |v: Vec<Vec<Option<&str>>>| Rows {
            columns: vec!["id".into(), "value".into()],
            values: v
                .into_iter()
                .map(|r| r.into_iter().map(|v| v.map(str::to_string)).collect())
                .collect(),
        };
        let a = rows(vec![vec![Some("1"), None], vec![Some("2"), Some("b")]]);
        let b = rows(vec![vec![Some("2"), Some("c")], vec![Some("1"), None]]);
        let result = data(&a, &s, &b, &s);
        assert!(result.keyed);
        assert_eq!(result.different, 1);
        assert_eq!(result.left.values[0][0].as_deref(), Some("="));
    }
    #[test]
    fn ddl_insert_does_not_shift_every_following_line() {
        let result = ddl("a\nb\nc", "a\nx\nb\nc").unwrap();
        assert_eq!(result.different, 1);
        assert_eq!(result.left.values.len(), 4);
        assert!(ddl(&"x\n".repeat(2001), "").is_err());
    }
    #[test]
    fn ddl_markers_separate_source_removals_target_additions_and_aligned_gaps() {
        let compared = ddl("a\nb\nc", "a\nx\nc").unwrap();
        assert_eq!(compared.different, 2);
        assert_eq!(compared.left.values[1][0].as_deref(), Some("-"));
        assert_eq!(compared.right.values[1][0].as_deref(), Some("∅"));
        assert_eq!(compared.right.values[2][0].as_deref(), Some("+"));
        assert_eq!(compared.left.values[2][0].as_deref(), Some("∅"));
    }
}

#[path = "schema_sync.rs"]
mod schema_sync;
pub use schema_sync::{SyncDraft, sync_ddl};

/// Align fields by exact identifier; unmatched rows remain visible on both sides.
pub fn fields(left: &Schema, right: &Schema) -> Result<Compared, String> {
    if left.columns.len() > 2000 || right.columns.len() > 2000 {
        return Err("compare.ddl_limit".into());
    }
    let names = left
        .columns
        .iter()
        .chain(&right.columns)
        .map(|c| c.name.clone())
        .collect::<BTreeSet<_>>();
    let columns = vec![
        "Δ",
        "ddl.column.name",
        "grid.structure.type",
        "ddl.column.not_null",
        "ddl.column.auto_increment",
        "ddl.column.primary",
        "ddl.column.extra",
        "ddl.column.comment",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let mut out = Compared {
        left: Rows {
            columns: columns.clone(),
            values: vec![],
        },
        right: Rows {
            columns,
            values: vec![],
        },
        different: 0,
        keyed: false,
    };
    let row = |c: &super::workbench::Column| {
        let flag = |v: Option<bool>| {
            Some(
                match v {
                    Some(true) => "✓",
                    Some(false) => "",
                    None => "—",
                }
                .to_string(),
            )
        };
        vec![
            Some(c.name.clone()),
            Some(c.data_type.clone()),
            flag(c.not_null),
            flag(c.auto_increment),
            flag(Some(c.primary)),
            Some(c.extra.clone()),
            Some(c.comment.clone()),
        ]
    };
    for name in names {
        let l = left.columns.iter().find(|c| c.name == name).map(row);
        let r = right.columns.iter().find(|c| c.name == name).map(row);
        let same = l == r;
        if !same {
            out.different += 1;
        }
        for (value, rows, marker) in [(l, &mut out.left, "-"), (r, &mut out.right, "+")] {
            let mut output = vec![Some(
                if value.is_none() {
                    "∅"
                } else if same {
                    "="
                } else {
                    marker
                }
                .to_string(),
            )];
            output.extend(value.unwrap_or_else(|| vec![None; 7]));
            rows.values.push(output);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod field_tests {
    use super::super::workbench::Column;
    use super::*;
    #[test]
    fn fields_align_names_and_keep_missing_and_changed_attributes() {
        let left = Schema {
            columns: vec![
                Column {
                    name: "id".into(),
                    data_type: "INT".into(),
                    ..Default::default()
                },
                Column {
                    name: "old".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let right = Schema {
            columns: vec![
                Column {
                    name: "id".into(),
                    data_type: "BIGINT".into(),
                    ..Default::default()
                },
                Column {
                    name: "new".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let result = fields(&left, &right).unwrap();
        assert_eq!(result.different, 3);
        assert_eq!(result.left.values.len(), result.right.values.len());
        assert_eq!(result.left.values[0][1].as_deref(), Some("id"));
        assert_eq!(result.left.values[1][0].as_deref(), Some("∅"));
        assert_eq!(result.right.values[2][0].as_deref(), Some("∅"));
    }
}
