use super::*;
use turbodbn_services::{
    DatabaseKind,
    database::workbench::{Action, Object, Reply},
};
impl NoteView {
    pub(crate) fn matches_table_query(
        &self,
        target: &crate::database::OpenTable,
        cx: &App,
    ) -> bool {
        if let Some(origin) = &self.note.table_query_origin {
            return origin.connection == target.connection
                && origin.database == target.database
                && origin.schema == target.object.schema
                && origin.table == target.object.name;
        }
        self.note.table_query.as_ref()
            == Some(&(target.object.schema.clone(), target.object.name.clone()))
            && self.note.default_connection_id == Some(target.connection)
            && self.default_database.read(cx).value().as_ref() == target.database
    }
    pub(crate) fn open_table_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.single_sql() {
            return;
        }
        if let Some(cell) = self.cells.first().cloned() {
            self.activate_cell(cell.entity_id(), window, cx);
            cell.update(cx, |cell, cx| cell.open_table_query(window, cx));
        }
    }
    pub(super) fn open_query_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.single_sql() {
            return;
        }
        let Some((schema, name)) = self.note.table_query.clone() else {
            return;
        };
        let snapshot = self.cells[0].read(cx).snapshot(cx);
        let CellContent::Sql {
            connection_id,
            database,
            ..
        } = snapshot.content
        else {
            return;
        };
        let Some(id) = connection_id.or(self.note.default_connection_id) else {
            feedback::error(t(cx, "ddl.choose_scope"), cx);
            return;
        };
        let db = database
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| self.default_database.read(cx).value().to_string());
        let Some(profile) = self.connections.read(cx).note_profile(id).cloned() else {
            return;
        };
        if !matches!(
            profile.kind,
            DatabaseKind::MySql
                | DatabaseKind::PostgreSql
                | DatabaseKind::Sqlite
                | DatabaseKind::ClickHouse
        ) {
            feedback::error(t(cx, "query_note.unsupported"), cx);
            return;
        }
        let generation = self.connections.read(cx).note_generation(id);
        let target = crate::database::OpenTable {
            connection: id,
            database: db.clone(),
            object: Object {
                schema,
                name,
                kind: "table".into(),
                comment: String::new(),
                parent: String::new(),
            },
            generation,
            kind: profile.kind,
            writable: profile.supports_mutation(),
            mysql_raw_gbk: profile.mysql_raw_gbk,
            mode: crate::table_view::Mode::Grid,
        };
        let revision = self.revision;
        let rx = self.connections.update(cx, |connections, cx| {
            connections.mcp_request(
                id,
                generation,
                db,
                Action::BrowseColumns(target.object.clone()),
                cx,
            )
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err("mcp.stale".into()));
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    if this.revision != revision
                        || this.connections.read(cx).note_generation(id) != generation
                    {
                        feedback::error(t(cx, "dbwork.context.changed"), cx);
                        return;
                    }
                    let schema = match result {
                        Ok(Reply::Schema(schema)) => schema,
                        Err(error) => {
                            feedback::error(crate::locale::message(cx, &error), cx);
                            return;
                        }
                        _ => return,
                    };
                    let filter = cx.new(|cx| {
                        crate::table_view::TableView::filter_builder(
                            this.connections.downgrade(),
                            target,
                            schema,
                            window,
                            cx,
                        )
                    });
                    this.query_filter_subscription = Some(cx.subscribe_in(
                        &filter,
                        window,
                        move |this, _, event: &crate::table_view::FilterSqlReady, window, cx| {
                            let sql = event.0.clone();
                            let owner = cx.entity().downgrade();
                            let revision = this.revision;
                            cx.defer_in(window, move |_, window, cx| {
                                window.close_dialog(cx);
                                feedback::confirm(
                                    t(cx, "query_note.apply"),
                                    format!("{}\n\n{sql}", t(cx, "query_note.replace")),
                                    window,
                                    cx,
                                    move |window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            if this.revision != revision
                                                || this.connections.read(cx).note_generation(id)
                                                    != generation
                                            {
                                                feedback::error(
                                                    t(cx, "dbwork.context.changed"),
                                                    cx,
                                                );
                                                return;
                                            }
                                            if let Some(cell) = this.cells.first() {
                                                cell.update(cx, |cell, cx| {
                                                    cell.replace_sql(sql.clone(), window, cx)
                                                });
                                            }
                                            this.changed(cx);
                                        });
                                    },
                                );
                            });
                        },
                    ));
                    this.query_filter = Some(filter.clone());
                    window.open_dialog(cx, move |dialog, _, cx| {
                        dialog
                            .title(t(cx, "query_note.filter"))
                            .w(px(700.))
                            .overlay_closable(false)
                            .child(filter.clone())
                            .child(feedback::actions().child(feedback::close_button(cx)))
                    });
                })
            });
        })
        .detach();
    }
}
