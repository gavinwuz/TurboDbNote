//! Bounded canonical Extended JSON presentation. Display summaries never replace source values.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const PAGE_BYTES: usize = 384 * 1024;
pub const DOCUMENT_BYTES: usize = 256 * 1024;
pub const MAX_FIELDS: usize = 256;
pub const MAX_NODES: usize = 10_000;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub documents: Vec<String>,
    pub offset: usize,
    pub more: bool,
    pub elapsed_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mutation {
    pub collection: String,
    pub original: String,
    pub replacement: String,
    pub clone_document: bool,
    pub confirmed: bool,
}
#[derive(Default)]
pub struct Projection {
    pub documents: Vec<Value>,
    pub fields: Vec<String>,
    pub types: Vec<String>,
    pub cells: Vec<Vec<Option<String>>>,
    pub limited: bool,
}
pub fn parse(source: &str) -> Result<Value, &'static str> {
    if source.len() > DOCUMENT_BYTES {
        return Err("mongo.document.limit");
    }
    let value: Value = serde_json::from_str(source).map_err(|_| "mongo.document.invalid")?;
    if !value.is_object() {
        return Err("mongo.document.invalid");
    }
    Ok(value)
}
pub fn project(page: &Page, previous: &[String]) -> Result<Projection, &'static str> {
    if page.documents.len() > 1000
        || page.documents.iter().map(String::len).sum::<usize>() > PAGE_BYTES
    {
        return Err("mongo.document.limit");
    }
    let mut result = Projection {
        fields: previous.iter().take(MAX_FIELDS).cloned().collect(),
        ..Projection::default()
    };
    if !result.fields.iter().any(|s| s == "_id") {
        result.fields.insert(0, "_id".into());
    }
    let mut seen: HashSet<String> = result.fields.iter().cloned().collect();
    for source in &page.documents {
        let value = parse(source)?;
        for key in value.as_object().unwrap().keys() {
            if !seen.contains(key) {
                if result.fields.len() < MAX_FIELDS {
                    seen.insert(key.clone());
                    result.fields.push(key.clone());
                } else {
                    result.limited = true;
                }
            }
        }
        result.documents.push(value);
    }
    result.types = result
        .fields
        .iter()
        .map(|key| {
            let types: HashSet<_> = result
                .documents
                .iter()
                .filter_map(|d| d.get(key))
                .map(kind)
                .collect();
            if types.len() == 1 {
                types.into_iter().next().unwrap().to_owned()
            } else if types.is_empty() {
                "Missing".into()
            } else {
                "Mixed".into()
            }
        })
        .collect();
    result.cells = result
        .documents
        .iter()
        .map(|doc| {
            result
                .fields
                .iter()
                .map(|key| doc.get(key).map(summary))
                .collect()
        })
        .collect();
    Ok(result)
}
pub fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "Null",
        Value::Bool(_) => "Boolean",
        Value::String(_) => "String",
        Value::Number(_) => "Number",
        Value::Array(_) => "Array",
        Value::Object(o) if o.len() == 1 => match o.keys().next().map(String::as_str) {
            Some("$oid") => "ObjectId",
            Some("$numberInt") => "Int32",
            Some("$numberLong") => "Int64",
            Some("$numberDouble") => "Double",
            Some("$numberDecimal") => "Decimal128",
            Some("$date") => "Date",
            Some("$binary") => "Binary",
            Some("$timestamp") => "Timestamp",
            Some("$regularExpression") => "Regex",
            Some("$minKey") => "MinKey",
            Some("$maxKey") => "MaxKey",
            Some("$code") => "JavaScript",
            Some("$symbol") => "Symbol",
            Some("$undefined") => "Undefined",
            Some("$dbPointer") => "DBPointer",
            _ => "Object",
        },
        Value::Object(o) if o.len() == 2 && o.contains_key("$code") && o.contains_key("$scope") => {
            "JavaScript"
        }
        Value::Object(_) => "Object",
    }
}
pub fn summary(value: &Value) -> String {
    let text = match value {
        Value::Array(a) => format!("[] {}", a.len()),
        Value::Object(o) if kind(value) == "Object" => format!("{{…}} {}", o.len()),
        Value::Object(o) if kind(value) == "ObjectId" => format!("ObjectId({})", o["$oid"]),
        Value::Object(o) if ["Int32", "Int64", "Double", "Decimal128"].contains(&kind(value)) => o
            .values()
            .next()
            .and_then(Value::as_str)
            .unwrap_or("?")
            .into(),
        Value::Object(_) => format!("{}(…)", kind(value)),
        Value::String(s) => s.chars().take(256).collect(),
        _ => value.to_string(),
    };
    text.chars().take(256).collect()
}
pub fn expandable(value: &Value) -> bool {
    value.is_array() || kind(value) == "Object"
}
pub fn child_path(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}
#[derive(Clone)]
pub struct Node {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub summary: String,
    pub kind: &'static str,
    pub expandable: bool,
}
pub fn nodes(
    document: &Value,
    open: &HashSet<String>,
    limits: &std::collections::HashMap<String, usize>,
) -> (Vec<Node>, bool) {
    struct Visitor<'a> {
        open: &'a HashSet<String>,
        limits: &'a std::collections::HashMap<String, usize>,
        out: Vec<Node>,
        limited: bool,
    }
    impl Visitor<'_> {
        fn visit(&mut self, value: &Value, path: String, name: String, depth: usize) {
            if self.out.len() >= MAX_NODES || depth > 64 {
                self.limited = true;
                return;
            }
            self.out.push(Node {
                path: path.clone(),
                name,
                depth,
                summary: summary(value),
                kind: kind(value),
                expandable: expandable(value),
            });
            if !self.open.contains(&path) {
                return;
            }
            let maximum = self.limits.get(&path).copied().unwrap_or(100);
            match value {
                Value::Object(map) if expandable(value) => {
                    if map.len() > maximum {
                        self.limited = true;
                    }
                    for (key, value) in map.iter().take(maximum) {
                        self.visit(value, child_path(&path, key), key.clone(), depth + 1);
                    }
                }
                Value::Array(array) => {
                    if array.len() > maximum {
                        self.limited = true;
                    }
                    for (i, value) in array.iter().enumerate().take(maximum) {
                        self.visit(
                            value,
                            child_path(&path, &i.to_string()),
                            format!("[{i}]"),
                            depth + 1,
                        );
                    }
                }
                _ => {}
            }
        }
    }
    let mut visitor = Visitor {
        open,
        limits,
        out: Vec::new(),
        limited: false,
    };
    visitor.visit(document, String::new(), "document".into(), 0);
    (visitor.out, visitor.limited)
}
pub fn apply(
    document: &Value,
    path: &str,
    replacement: Option<Value>,
) -> Result<Value, &'static str> {
    if path == "/_id" || path.starts_with("/_id/") {
        return Err("mongo.document.id_immutable");
    }
    let mut result = document.clone();
    if path.is_empty() {
        result = replacement.ok_or("mongo.document.invalid")?;
    } else {
        let (parent, key) = path.rsplit_once('/').ok_or("mongo.document.invalid")?;
        let key = key.replace("~1", "/").replace("~0", "~");
        let parent = result.pointer_mut(parent).ok_or("mongo.document.invalid")?;
        match parent {
            Value::Object(map) => {
                if !map.contains_key(&key) {
                    return Err("mongo.document.invalid");
                }
                if let Some(value) = replacement {
                    map.insert(key, value);
                } else {
                    map.remove(&key);
                }
            }
            Value::Array(array) => {
                let index = key.parse::<usize>().map_err(|_| "mongo.document.invalid")?;
                if index >= array.len() {
                    return Err("mongo.document.invalid");
                }
                if let Some(value) = replacement {
                    array[index] = value;
                } else {
                    array.remove(index);
                }
            }
            _ => return Err("mongo.document.invalid"),
        }
    }
    if !result.is_object() || result.get("_id") != document.get("_id") {
        return Err("mongo.document.id_immutable");
    }
    if result.to_string().len() > DOCUMENT_BYTES {
        return Err("mongo.document.limit");
    }
    Ok(result)
}
pub fn validate_mutation(change: &Mutation) -> Result<(Value, Value), &'static str> {
    if !change.confirmed
        || change.collection.is_empty()
        || change.collection.len() > 1024
        || change.collection.starts_with("system.")
        || change.collection.chars().any(char::is_control)
    {
        return Err("mongo.document.denied");
    }
    let old = parse(&change.original)?;
    let new = parse(&change.replacement)?;
    if change.clone_document {
        if new.get("_id").is_some() {
            return Err("mongo.document.id_immutable");
        }
    } else if old.get("_id").is_none() || old.get("_id") != new.get("_id") {
        return Err("mongo.document.id_immutable");
    }
    Ok((old, new))
}
/// Bounded display conversion, never evaluated or sent to a server.
pub fn shell(value: &Value) -> String {
    match value {
        Value::Object(o) if kind(value) == "ObjectId" => format!("ObjectId({})", o["$oid"]),
        Value::Object(o) if kind(value) == "Int64" => format!("NumberLong({})", o["$numberLong"]),
        Value::Object(o) if kind(value) == "Int32" => format!("NumberInt({})", o["$numberInt"]),
        Value::Object(o) if kind(value) == "Decimal128" => {
            format!("NumberDecimal({})", o["$numberDecimal"])
        }
        Value::Object(_) if kind(value) != "Object" => format!(
            "EJSON.parse({})",
            serde_json::to_string(&value.to_string()).unwrap()
        ),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", serde_json::to_string(k).unwrap(), shell(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(a) => format!("[{}]", a.iter().map(shell).collect::<Vec<_>>().join(", ")),
        _ => value.to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn typed_projection_is_bounded_and_lossless() {
        let page=Page{documents:vec![json!({"_id":{"$oid":"012345678901234567890123"},"n":{"$numberLong":"9223372036854775807"},"a":[1,2],"nullable":null}).to_string(),json!({"n":{"$numberInt":"2"},"extra":true}).to_string()],offset:0,more:false,elapsed_ms:0};
        let p = project(&page, &[]).unwrap();
        assert_eq!(p.fields[0], "_id");
        assert_eq!(
            p.types[p.fields.iter().position(|s| s == "n").unwrap()],
            "Mixed"
        );
        assert_eq!(
            p.cells[0][p.fields.iter().position(|s| s == "n").unwrap()].as_deref(),
            Some("9223372036854775807")
        );
        let c = p.fields.iter().position(|s| s == "nullable").unwrap();
        assert_eq!(p.cells[0][c].as_deref(), Some("null"));
        assert!(p.cells[1][c].is_none());
        let second = project(
            &Page {
                documents: vec![json!({"fresh":1}).to_string()],
                ..page
            },
            &p.fields,
        )
        .unwrap();
        assert!(second.fields.starts_with(&p.fields));
    }
    #[test]
    fn nodes_expand_only_visible_children_and_paths_are_unambiguous() {
        let d = json!({"a/b~c":(0..1000).collect::<Vec<_>>()});
        let path = child_path("", "a/b~c");
        let open = HashSet::from([String::new(), path.clone()]);
        let (rows, limited) = nodes(&d, &open, &Default::default());
        assert_eq!(rows.len(), 102);
        assert!(limited);
        assert_eq!(d.pointer(&path).unwrap().as_array().unwrap().len(), 1000);
        assert_eq!(nodes(&d, &HashSet::new(), &Default::default()).0.len(), 1);
    }
    #[test]
    fn edits_preserve_id_and_reject_unconfirmed_or_oversized_writes() {
        let old = json!({"_id":{"$numberInt":"1"},"a/b":1,"a":[1,2]});
        let edited = apply(&old, "/a~1b", Some(json!(3))).unwrap();
        assert_eq!(edited["a/b"], 3);
        assert_eq!(old["a/b"], 1);
        assert!(apply(&old, "/_id", Some(json!(2))).is_err());
        assert!(apply(&old, "", Some(json!({"a":1}))).is_err());
        let m = Mutation {
            collection: "c".into(),
            original: old.to_string(),
            replacement: edited.to_string(),
            clone_document: false,
            confirmed: false,
        };
        assert!(validate_mutation(&m).is_err());
        assert!(
            validate_mutation(&Mutation {
                confirmed: true,
                ..m
            })
            .is_ok()
        );
    }
}
