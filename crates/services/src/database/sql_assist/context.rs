//! Tolerant lexical query scopes for incomplete editor text. Never used for execution.
use super::*;

#[derive(Clone)]
struct Binding {
    name: String,
    table: Option<TableName>,
    columns: Option<Vec<String>>,
}
struct Scope {
    start: usize,
    end: usize,
    parent: Option<usize>,
    tokens: Vec<usize>,
    bindings: Vec<Binding>,
    ctes: Vec<Binding>,
}
struct Context<'a> {
    tokens: Vec<Piece<'a>>,
    scopes: Vec<Scope>,
    owners: Vec<usize>,
    pairs: Vec<Option<usize>>,
    kind: Option<DatabaseKind>,
}
fn keyword(token: &Token, value: &str) -> bool {
    matches!(token, Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(value))
}
fn ident(kind: Option<DatabaseKind>, token: &Token) -> Option<String> {
    let Token::Word(w) = token else {
        return None;
    };
    if w.value.len() > 256 || w.value.chars().any(char::is_control) {
        return None;
    }
    Some(
        if kind == Some(DatabaseKind::Sqlite)
            || kind == Some(DatabaseKind::PostgreSql) && w.quote_style.is_none()
        {
            w.value.to_lowercase()
        } else {
            w.value.clone()
        },
    )
}
impl<'a> Context<'a> {
    fn new(kind: Option<DatabaseKind>, sql: &'a str) -> Option<Self> {
        if sql.len() > MAX_SOURCE || kind == Some(DatabaseKind::Redis) {
            return None;
        }
        let tokens = pieces(kind, sql)
            .ok()?
            .into_iter()
            .filter(|p| !matches!(p.token, Token::Whitespace(_)))
            .collect::<Vec<_>>();
        let mut scopes = vec![Scope {
            start: 0,
            end: sql.len(),
            parent: None,
            tokens: vec![],
            bindings: vec![],
            ctes: vec![],
        }];
        let mut owners = vec![0; tokens.len()];
        let mut pairs = vec![None; tokens.len()];
        let mut stack = vec![];
        let mut scope = 0;
        for (i, piece) in tokens.iter().enumerate() {
            owners[i] = scope;
            scopes[scope].tokens.push(i);
            match piece.token {
                Token::LParen => {
                    let query = tokens
                        .get(i + 1)
                        .is_some_and(|p| keyword(&p.token, "SELECT") || keyword(&p.token, "WITH"));
                    stack.push((i, scope, query));
                    if stack.len() > 64 {
                        return None;
                    }
                    if query {
                        let child = scopes.len();
                        scopes.push(Scope {
                            start: piece.end,
                            end: sql.len(),
                            parent: Some(scope),
                            tokens: vec![],
                            bindings: vec![],
                            ctes: vec![],
                        });
                        scope = child;
                    }
                }
                Token::RParen => {
                    if let Some((open, parent, query)) = stack.pop() {
                        pairs[open] = Some(i);
                        pairs[i] = Some(open);
                        if query {
                            scopes[scope].end = piece.start;
                            scope = parent;
                        }
                    }
                }
                _ => {}
            }
        }
        let count = scopes.len();
        for parent in 0..count {
            let original = scopes[parent].tokens.clone();
            let split = original
                .iter()
                .copied()
                .filter(|&i| {
                    keyword(&tokens[i].token, "UNION")
                        || keyword(&tokens[i].token, "EXCEPT")
                        || keyword(&tokens[i].token, "INTERSECT")
                })
                .collect::<Vec<_>>();
            if split.is_empty() {
                continue;
            }
            let Some(first) = original
                .iter()
                .copied()
                .find(|&i| keyword(&tokens[i].token, "SELECT"))
            else {
                continue;
            };
            let mut starts = vec![first];
            for separator in &split {
                if let Some(next) = original
                    .iter()
                    .copied()
                    .find(|&i| i > *separator && keyword(&tokens[i].token, "SELECT"))
                {
                    starts.push(next);
                }
            }
            for (position, start) in starts.iter().copied().enumerate() {
                let end = split
                    .get(position)
                    .map_or(scopes[parent].end, |&i| tokens[i].start);
                let arm = scopes.len();
                let indices = original
                    .iter()
                    .copied()
                    .filter(|&i| tokens[i].start >= tokens[start].start && tokens[i].start < end)
                    .collect::<Vec<_>>();
                for &i in &indices {
                    owners[i] = arm;
                }
                for child in scopes.iter_mut().take(count) {
                    if child.parent == Some(parent)
                        && child.start >= tokens[start].start
                        && child.end <= end
                    {
                        child.parent = Some(arm);
                    }
                }
                scopes.push(Scope {
                    start: tokens[start].start,
                    end,
                    parent: Some(parent),
                    tokens: indices,
                    bindings: vec![],
                    ctes: vec![],
                });
            }
            scopes[parent].tokens.retain(|&i| i < first);
        }
        let mut context = Self {
            tokens,
            scopes,
            owners,
            pairs,
            kind,
        };
        // Bind inner projections first, then resolve named CTEs at lookup time.
        for scope in (0..context.scopes.len()).rev() {
            context.bind(scope);
        }
        Some(context)
    }
    fn token(&self, i: usize) -> Option<&Token> {
        self.tokens.get(i).map(|p| &p.token)
    }
    fn word(&self, i: usize) -> Option<String> {
        ident(self.kind, self.token(i)?)
    }
    fn is(&self, i: usize, word: &str) -> bool {
        self.token(i).is_some_and(|t| keyword(t, word))
    }
    fn alias(&self, mut i: usize) -> (Option<String>, usize) {
        if self.is(i, "AS") {
            i += 1;
            return (self.word(i), i + 1);
        }
        let name = self.token(i).and_then(|t| match t {
            Token::Word(w)
                if w.quote_style.is_some()
                    || w.keyword == sqlparser::keywords::Keyword::NoKeyword =>
            {
                self.word(i)
            }
            _ => None,
        });
        let next = i + usize::from(name.is_some());
        (name, next)
    }
    fn projection(&self, scope: usize) -> Option<Vec<String>> {
        let indices = &self.scopes[scope].tokens;
        let start = indices.iter().position(|&i| self.is(i, "SELECT"))? + 1;
        let end = indices
            .iter()
            .position(|&i| self.is(i, "FROM"))
            .unwrap_or(indices.len());
        let mut names = vec![];
        let mut expression = vec![];
        let mut depth = 0usize;
        for &i in &indices[start..end] {
            match self.tokens[i].token {
                Token::LParen => depth += 1,
                Token::RParen => depth = depth.saturating_sub(1),
                Token::Mul if depth == 0 => return None,
                Token::Comma if depth == 0 => {
                    names.push(self.output_name(&expression)?);
                    expression.clear();
                    continue;
                }
                _ => {}
            }
            expression.push(i);
        }
        if !expression.is_empty() {
            names.push(self.output_name(&expression)?);
        }
        (!names.is_empty()).then_some(names)
    }
    fn output_name(&self, indices: &[usize]) -> Option<String> {
        let &last = indices.last()?;
        // Explicit AS or a simple (qualified) column. Unknown expressions are not invented.
        if indices.len() >= 2 && self.is(indices[indices.len() - 2], "AS") {
            return self.word(last);
        }
        if indices
            .iter()
            .all(|&i| matches!(self.tokens[i].token, Token::Word(_) | Token::Period))
        {
            return self.word(last);
        }
        None
    }
    fn output(&self, scope: usize) -> (Option<TableName>, Option<Vec<String>>) {
        if let Some(columns) = self.projection(scope) {
            return (None, Some(columns));
        }
        let indices = &self.scopes[scope].tokens;
        let projection = indices
            .iter()
            .copied()
            .skip_while(|&i| !self.is(i, "SELECT"))
            .skip(1)
            .take_while(|&i| !self.is(i, "FROM"))
            .collect::<Vec<_>>();
        if projection.len() == 1
            && matches!(self.tokens[projection[0]].token, Token::Mul)
            && self.scopes[scope].bindings.len() == 1
        {
            let source = &self.scopes[scope].bindings[0];
            return (source.table.clone(), source.columns.clone());
        }
        (None, None)
    }
    fn bind(&mut self, scope: usize) {
        let indices = self.scopes[scope].tokens.clone();
        let mut from = false;
        let mut depth = 0usize;
        let mut from_depth = 0usize;
        for &i in &indices {
            match self.tokens[i].token {
                Token::LParen => depth += 1,
                Token::RParen => depth = depth.saturating_sub(1),
                _ => {}
            }
            if self.is(i, "WHERE")
                || self.is(i, "GROUP")
                || self.is(i, "ORDER")
                || self.is(i, "HAVING")
                || self.is(i, "SET")
            {
                from = false;
            }
            // CTE name [optional output names] AS (query).
            if self.is(i, "AS") && matches!(self.token(i + 1), Some(Token::LParen)) {
                let open = i + 1;
                if let Some(child) = self
                    .scopes
                    .iter()
                    .position(|s| s.start == self.tokens[open].end)
                {
                    let mut name = i.checked_sub(1).and_then(|j| self.word(j));
                    let (mut table, mut columns) = self.output(child);
                    if matches!(self.token(i.saturating_sub(1)), Some(Token::RParen))
                        && let Some(left) = self.pairs[i - 1]
                    {
                        name = left.checked_sub(1).and_then(|j| self.word(j));
                        columns = Some((left + 1..i - 1).filter_map(|j| self.word(j)).collect());
                        table = None;
                    }
                    if let Some(name) = name {
                        self.scopes[scope].ctes.push(Binding {
                            name,
                            table,
                            columns,
                        });
                    }
                }
            }
            let relation = self.is(i, "FROM")
                || self.is(i, "JOIN")
                || self.is(i, "UPDATE")
                || self.is(i, "INTO")
                || self.is(i, "USING");
            let comma = from && depth == from_depth && matches!(self.token(i), Some(Token::Comma));
            if !relation && !comma {
                continue;
            }
            from = true;
            from_depth = depth;
            let first = i + 1 + usize::from(self.is(i + 1, "LATERAL") || self.is(i + 1, "ONLY"));
            if self.owners.get(first) != Some(&scope) {
                continue;
            }
            if matches!(self.token(first), Some(Token::LParen)) {
                if let Some(close) = self.pairs[first] {
                    let (alias, _) = self.alias(close + 1);
                    if let Some(name) = alias {
                        let (table, columns) = self
                            .scopes
                            .iter()
                            .position(|s| s.start == self.tokens[first].end)
                            .map(|child| self.output(child))
                            .unwrap_or((None, None));
                        self.scopes[scope].bindings.push(Binding {
                            name,
                            table,
                            columns,
                        });
                    }
                }
                continue;
            }
            let Some(mut name) = self.word(first) else {
                continue;
            };
            let mut schema = String::new();
            let mut next = first + 1;
            if matches!(self.token(next), Some(Token::Period)) {
                let Some(table) = self.word(next + 1) else {
                    continue;
                };
                schema = name;
                name = table;
                next += 2;
            }
            if matches!(self.token(next), Some(Token::LParen)) {
                if let Some(close) = self.pairs[next]
                    && let (Some(name), _) = self.alias(close + 1)
                {
                    self.scopes[scope].bindings.push(Binding {
                        name,
                        table: None,
                        columns: None,
                    });
                }
                continue;
            } // table function outputs cannot be inferred from base-table metadata
            let (alias, _) = self.alias(next);
            self.scopes[scope].bindings.push(Binding {
                name: alias.unwrap_or_else(|| name.clone()),
                table: Some(TableName { schema, name }),
                columns: None,
            });
        }
    }
    fn scope(&self, offset: usize) -> usize {
        self.scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.start <= offset && offset <= s.end)
            .min_by_key(|(_, s)| s.end.saturating_sub(s.start))
            .map_or(0, |(i, _)| i)
    }
    fn resolve(&self, mut scope: usize, alias: &str) -> Option<Binding> {
        loop {
            let current = &self.scopes[scope];
            let matches = current
                .bindings
                .iter()
                .filter(|b| b.name == alias)
                .collect::<Vec<_>>();
            if !matches.is_empty() {
                if matches.len() != 1 {
                    return None;
                }
                let mut binding = matches[0].clone();
                let mut seen = std::collections::HashSet::new();
                for _ in 0..64 {
                    let Some(table) = &binding.table else {
                        break;
                    };
                    if !table.schema.is_empty() {
                        break;
                    }
                    let mut cte_scope = Some(scope);
                    let mut found = None;
                    while let Some(s) = cte_scope {
                        if let Some(cte) = self.scopes[s].ctes.iter().find(|c| c.name == table.name)
                        {
                            found = Some((s, cte));
                            break;
                        }
                        cte_scope = self.scopes[s].parent;
                    }
                    let Some((s, cte)) = found else {
                        break;
                    };
                    if !seen.insert((s, cte.name.clone())) {
                        return None;
                    }
                    binding.columns = cte.columns.clone();
                    binding.table = cte.table.clone();
                }
                return Some(binding);
            }
            scope = current.parent?;
        }
    }
}
pub(super) fn column_request(
    kind: Option<DatabaseKind>,
    sql: &str,
    offset: usize,
) -> Option<ColumnRequest> {
    if !sql.is_char_boundary(offset) {
        return None;
    }
    let ctx = Context::new(kind, sql)?;
    let start = sql[..offset]
        .char_indices()
        .rev()
        .find(|(_, c)| !word(*c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = sql[offset..]
        .char_indices()
        .find(|(_, c)| !word(*c))
        .map_or(sql.len(), |(i, _)| offset + i);
    let dot = ctx.tokens.iter().rposition(|p| p.end <= start)?;
    if !matches!(ctx.tokens[dot].token, Token::Period) || ctx.tokens[dot].end != start {
        return None;
    }
    let alias = ctx.word(dot.checked_sub(1)?)?;
    // A dot inside a literal/comment is never a qualified identifier token.
    if ctx.tokens[dot].start >= offset {
        return None;
    }
    let binding = ctx.resolve(ctx.scope(offset), &alias)?;
    if binding.table.is_none() && binding.columns.is_none() {
        return None;
    }
    let table = binding.table.unwrap_or(TableName {
        schema: String::new(),
        name: alias,
    });
    Some(ColumnRequest {
        table,
        local_columns: binding.columns,
        prefix: sql[start..offset].into(),
        range: start..end,
    })
}
pub(super) fn tables(kind: Option<DatabaseKind>, sql: &str, offset: usize) -> Vec<TableName> {
    let Some(ctx) = Context::new(kind, sql) else {
        return vec![];
    };
    let scope = ctx.scope(offset);
    let mut tables = vec![];
    for binding in &ctx.scopes[scope].bindings {
        if let Some(resolved) = ctx.resolve(scope, &binding.name)
            && let Some(table) = resolved.table
            && !tables.contains(&table)
            && tables.len() < 8
        {
            tables.push(table);
        }
    }
    tables
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_expressions_dml_and_scope_boundaries() {
        for (sql, table) in [
            ("SELECT u.| u.id FROM users u", "users"),
            ("SELECT CASE WHEN u.| FROM users u", "users"),
            ("SELECT * FROM users u WHERE CASE WHEN u.|", "users"),
            ("SELECT coalesce(u.| FROM users u", "users"),
            ("SELECT * FROM users u GROUP BY u.|", "users"),
            ("SELECT * FROM users u HAVING u.|", "users"),
            ("SELECT * FROM users u ORDER BY u.| DESC", "users"),
            ("SELECT * FROM users u JOIN teams t ON t.| AND", "teams"),
            (
                "SELECT u.id FROM users u UNION ALL SELECT (SELECT CASE WHEN u.| FROM teams u) FROM users x",
                "teams",
            ),
            ("UPDATE users u SET u.|", "users"),
            (
                "SELECT u.| FROM users u UNION ALL SELECT u.id FROM teams u",
                "users",
            ),
            (
                "SELECT u.id FROM users u UNION ALL SELECT u.| FROM teams u",
                "teams",
            ),
            ("DELETE FROM users u WHERE u.|", "users"),
            (
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM teams t WHERE u.| AND",
                "users",
            ),
            (
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM teams u WHERE u.| AND",
                "teams",
            ),
        ] {
            let offset = sql.find('|').unwrap();
            let sql = sql.replace('|', "");
            assert_eq!(
                super::super::column_request(Some(DatabaseKind::MySql), &sql, offset)
                    .map(|r| r.table.name),
                Some(table.into()),
                "{sql}"
            );
        }
        for sql in [
            "SELECT * FROM users u JOIN teams u ON u.|",
            "SELECT * FROM users u WHERE 'u.|'",
            "SELECT * FROM users u -- u.|",
            "SELECT * FROM users u /* u.| */",
        ] {
            let offset = sql.find('|').unwrap();
            let sql = sql.replace('|', "");
            assert!(
                column_request(Some(DatabaseKind::MySql), &sql, offset).is_none(),
                "{sql}"
            );
        }
    }
    #[test]
    fn virtual_outputs_are_local_and_not_base_table_guesses() {
        for sql in [
            "WITH x AS (SELECT id, name AS label FROM users) SELECT x.| FROM x",
            "SELECT x.| FROM (SELECT id, name AS label FROM users) x",
        ] {
            let offset = sql.find('|').unwrap();
            let sql = sql.replace('|', "");
            let request = column_request(Some(DatabaseKind::MySql), &sql, offset).unwrap();
            assert_eq!(
                request.local_columns,
                Some(vec!["id".into(), "label".into()])
            );
        }
    }
}
