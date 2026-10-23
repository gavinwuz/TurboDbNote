use gpui::*;
use std::collections::{HashMap, HashSet, VecDeque};
use turbodbn_services::database::{
    catalog_browser::{MAX_CACHE, PAGE},
    workbench::{Action, Column, Object},
};
#[derive(Clone)]
pub(super) enum TreeRow {
    Category(&'static str),
    Object(Object),
    Page(String),
}
impl TreeRow {
    pub fn key(&self) -> String {
        match self {
            Self::Category(c) => format!("category:{c}"),
            Self::Object(o) => key(o),
            Self::Page(c) => format!("page:{c}"),
        }
    }
}
pub(super) fn key(o: &Object) -> String {
    serde_json::to_string(&(&o.kind, &o.schema, &o.parent, &o.name)).unwrap_or_default()
}
pub(super) fn bucket(category: &str) -> &str {
    if matches!(category, "table" | "view") {
        "relations"
    } else {
        category
    }
}
#[derive(Default)]
pub(super) struct PageState {
    pub loaded: bool,
    pub more: bool,
    pub offset: usize,
}
pub(super) struct Browser {
    pub supported: bool,
    pub all_categories: bool,
    pub include_comments: bool,
    pub channel: u64,
    pub task: uuid::Uuid,
    pub queued_ms: u64,
    pub execution_timeout: std::time::Duration,
    pub session: Option<turbodbn_services::mysql::Session>,
    pub worker: Option<Task<()>>,
    pub deadline: Option<Task<()>>,
    pub idle: Option<Task<()>>,
    pub ready: bool,
    pub sent: bool,
    pub started: Option<std::time::Instant>,
    pub queued: HashMap<String, std::time::Instant>,
    pub query: String,
    pub pages: HashMap<(String, String), PageState>,
    pub queue: VecDeque<Action>,
    pub pending: Option<Action>,
    pub errors: HashMap<String, String>,
    pub columns: HashSet<String>,
    pub full: HashSet<String>,
    pub sections: HashSet<String>,
    pub pending_edit: Option<(Object, Column)>,
    pub timer: Option<Task<()>>,
    pub list: ListState,
    pub layout: Vec<String>,
    pub revision: u64,
    pub rows: std::rc::Rc<Vec<TreeRow>>,
    pub fingerprint: Option<(u64, String, u64)>,
    pub counts: HashMap<String, usize>,
    pub searches: HashMap<String, (String, String)>,
    pub unknown_limit: bool,
}
impl Default for Browser {
    fn default() -> Self {
        Self {
            supported: false,
            all_categories: false,
            include_comments: false,
            channel: 0,
            task: uuid::Uuid::new_v4(),
            queued_ms: 0,
            execution_timeout: std::time::Duration::from_secs(300),
            session: None,
            worker: None,
            deadline: None,
            idle: None,
            ready: false,
            sent: false,
            started: None,
            queued: HashMap::new(),
            query: String::new(),
            pages: HashMap::new(),
            queue: VecDeque::new(),
            pending: None,
            errors: HashMap::new(),
            columns: HashSet::new(),
            full: HashSet::new(),
            sections: HashSet::new(),
            pending_edit: None,
            timer: None,
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            layout: Vec::new(),
            revision: 0,
            rows: Default::default(),
            fingerprint: None,
            counts: HashMap::new(),
            searches: HashMap::new(),
            unknown_limit: false,
        }
    }
}
impl Browser {
    pub fn reset(&mut self) {
        let rev = self.revision + 1;
        *self = Self::default();
        self.revision = rev;
    }
    pub fn token(a: &Action) -> String {
        match a {
            Action::BrowseCatalog {
                category,
                search,
                offset,
                ..
            } => format!("page:{category}:{search}:{offset}"),
            Action::BrowseColumns(o) => format!("columns:{}", key(o)),
            Action::BrowseSection { object, section } => {
                format!("section:{}:{section}", key(object))
            }
            Action::Describe(o) => format!("full:{}", key(o)),
            _ => "other".into(),
        }
    }
    pub fn push(&mut self, a: Action) {
        let token = Self::token(&a);
        if self
            .pending
            .as_ref()
            .is_some_and(|p| Self::token(p) == token)
            || self.queue.iter().any(|p| Self::token(p) == token)
        {
            return;
        }
        if self.queue.len() >= 64 {
            self.errors.insert(token, "browser.queue_limit".into());
            return;
        }
        if self.queue.len() < 64 {
            self.errors.remove(&token);
            self.queued.insert(token, std::time::Instant::now());
            if matches!(a, Action::BrowseColumns(_)) {
                self.queue.push_front(a);
            } else {
                self.queue.push_back(a);
            }
            self.revision += 1;
        }
    }
    pub fn page(&mut self, category: &str, query: &str, more: bool) {
        let k = (category.to_owned(), query.to_owned());
        let offset = if more {
            self.pages.get(&k).map_or(0, |p| p.offset)
        } else {
            0
        };
        self.push(Action::BrowseCatalog {
            category: category.into(),
            search: query.into(),
            offset,
            limit: PAGE,
            include_comments: self.include_comments,
        });
    }
    pub fn search(&mut self, query: String, categories: &[&str]) {
        self.pages.retain(|(_, q), _| q.is_empty() || *q == query);
        self.errors.retain(|k, _| !k.starts_with("page:"));
        self.query = query.clone();
        self.queue
            .retain(|a| !matches!(a, Action::BrowseCatalog { .. }));
        self.queued.retain(|k, _| !k.starts_with("page:"));
        let mut seen = HashSet::new();
        for c in categories {
            let c = bucket(c);
            if !seen.insert(c) {
                continue;
            }
            let k = (c.to_owned(), query.clone());
            let complete_base = !self.include_comments
                && !self.unknown_limit
                && self
                    .pages
                    .get(&(c.to_owned(), String::new()))
                    .is_some_and(|p| p.loaded && !p.more);
            if !complete_base && !self.pages.get(&k).is_some_and(|p| p.loaded) {
                self.page(c, &query, false);
            }
        }
        self.revision += 1;
    }
    pub fn matches(&self, o: &Object, query: &str) -> bool {
        query.is_empty()
            || self.searches.get(&key(o)).is_some_and(|(name, comment)| {
                name.contains(query)
                    || (self.include_comments || !self.supported) && comment.contains(query)
            })
    }
    pub fn cache_search(&mut self, object: &Object) {
        self.searches.insert(
            key(object),
            (
                format!("{} {} {}", object.schema, object.name, object.parent).to_lowercase(),
                object.comment.to_lowercase(),
            ),
        );
    }
    pub fn merge(&mut self, target: &mut Vec<Object>, objects: Vec<Object>) {
        let mut positions = target
            .iter()
            .enumerate()
            .map(|(i, o)| (key(o), i))
            .collect::<HashMap<_, _>>();
        for o in objects {
            let k = key(&o);
            if let Some(index) = positions.get(&k).copied() {
                let mut o = o;
                if o.comment.is_empty() {
                    o.comment.clone_from(&target[index].comment);
                }
                self.cache_search(&o);
                target[index] = o;
            } else {
                positions.insert(k, target.len());
                self.cache_search(&o);
                target.push(o);
            }
        }
        target.sort_by(|a, b| {
            (&a.schema, &a.name, &a.kind, &a.parent).cmp(&(&b.schema, &b.name, &b.kind, &b.parent))
        });
        if target.len() > MAX_CACHE {
            let excess = target.len() - MAX_CACHE;
            for o in target.drain(..excess) {
                self.searches.remove(&key(&o));
            }
            self.pages.retain(|(_, q), _| !q.is_empty());
            self.unknown_limit = true;
        }
        self.revision += 1;
    }
    pub fn loading(&self, category: &str, query: &str) -> bool {
        self.pending.iter().chain(self.queue.iter()).any(|a|matches!(a,Action::BrowseCatalog{category:c,search,..} if c==category&&search==query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn object(n: usize) -> Object {
        Object {
            schema: "s".into(),
            name: format!("item_{n:04}"),
            kind: "table".into(),
            parent: String::new(),
            comment: String::new(),
        }
    }
    #[::core::prelude::v1::test]
    fn cache_is_bounded_deduplicated_and_search_is_not_blocked_by_a_local_hit() {
        let mut b = Browser::default();
        let mut objects = vec![];
        b.merge(&mut objects, vec![object(0)]);
        b.pages.insert(
            ("relations".into(), String::new()),
            PageState {
                loaded: true,
                more: true,
                offset: 1,
            },
        );
        b.search("item".into(), &["table", "view"]);
        assert_eq!(b.queue.len(), 1);
        b.merge(&mut objects, vec![object(0), object(1000)]);
        assert_eq!(objects.len(), 2);
        b.merge(&mut objects, (1..=5100).map(object).collect());
        assert_eq!(objects.len(), 5000);
        assert!(b.unknown_limit);
        assert!(b.matches(&object(5100), "5100"));
        b.reset();
        assert!(b.queue.is_empty());
        assert!(b.pages.is_empty());
        assert!(b.rows.is_empty());
    }
    #[::core::prelude::v1::test]
    fn a_complete_cached_category_does_not_issue_another_remote_search() {
        let mut b = Browser::default();
        b.pages.insert(
            ("relations".into(), String::new()),
            PageState {
                loaded: true,
                more: false,
                offset: 10,
            },
        );
        b.search("name".into(), &["table", "view"]);
        assert!(b.queue.is_empty());
        let a = Object {
            schema: "a:b".into(),
            name: "c".into(),
            kind: "table".into(),
            parent: String::new(),
            comment: String::new(),
        };
        let mut other = a.clone();
        other.schema = "a".into();
        other.name = "b:c".into();
        assert_ne!(key(&a), key(&other));
    }
    #[::core::prelude::v1::test]
    fn slow_typing_keeps_only_latest_search_and_columns_have_priority() {
        let mut b = Browser::default();
        for word in ["c", "cq", "cq_", "cq_itemtype"] {
            b.search(word.into(), &["table", "view"]);
            assert_eq!(b.queue.len(), 1);
            assert!(
                matches!(b.queue.front(), Some(Action::BrowseCatalog{search,..}) if search == word)
            );
        }
        let col = Action::BrowseColumns(object(8));
        b.push(col.clone());
        b.push(col);
        assert_eq!(b.queue.len(), 2);
        assert!(matches!(b.queue.front(), Some(Action::BrowseColumns(_))));
        b.search("cq_itemtype2".into(), &["table", "view"]);
        assert!(matches!(b.queue.front(), Some(Action::BrowseColumns(_))));
        assert_eq!(b.queued.len(), 2);
    }
    #[::core::prelude::v1::test]
    fn comment_search_is_explicit_and_names_pages_preserve_cached_comments() {
        let mut b = Browser {
            supported: true,
            ..Default::default()
        };
        let mut objects = vec![];
        let mut o = object(1);
        o.comment = "测试说明".into();
        b.merge(&mut objects, vec![o.clone()]);
        assert!(!b.matches(&o, "测试"));
        b.include_comments = true;
        assert!(b.matches(&o, "测试"));
        o.comment.clear();
        b.merge(&mut objects, vec![o.clone()]);
        assert!(b.matches(&o, "测试"));
        assert_eq!(objects[0].comment, "测试说明");
    }
}
