use super::authorization::{ReviewState, SqlReview};
use super::*;
use crate::{feedback, locale::t};
use turbodbn_services::{
    database::{
        notebook_query,
        workbench::{Action, Reply},
    },
    sql_mcp,
};

impl ChatView {
    pub(super) fn start_sql_mcp(&mut self, cx: &mut Context<Self>) -> Option<sql_mcp::Config> {
        let (server, calls) = match sql_mcp::Server::start() {
            Ok(value) => value,
            Err(_) => {
                feedback::error(t(cx, "mcp.unavailable"), cx);
                return None;
            }
        };
        let config = server.config.clone();
        let generation = self.generation;
        self.mcp_server = Some(server);
        self.mcp_task = Some(cx.spawn(async move |view, cx| {
            while let Ok(call) = calls.recv().await {
                let started = std::time::Instant::now();
                let initial = view.update(cx, |this, cx| {
                    this.ready_sql_connection(cx);
                    (this.sql_scope.clone(), this.started)
                }).ok();
                loop {
                    let waiting = view.update(cx, |this, cx| {
                        if initial.as_ref() != Some(&(this.sql_scope.clone(), this.started)) || this.generation != generation || this.stopping || call.response.is_closed() { return false; }
                        this.sql_scope.as_ref().and_then(|s| s.connection).and_then(|id| this.databases.as_ref().and_then(|db| db.upgrade()).map(|db| db.read(cx).note_connection_state(id).is_none())).unwrap_or(false)
                    }).unwrap_or(false);
                    if !waiting || started.elapsed() >= std::time::Duration::from_secs(60) { break; }
                    gpui::Timer::after(std::time::Duration::from_millis(100)).await;
                }
                let setup = view.update(cx, |this, cx| {
                    if this.generation != generation || initial.as_ref() != Some(&(this.sql_scope.clone(), this.started)) || call.response.is_closed() { return Err("mcp.stale"); }
                    let scope = this.sql_scope.clone().ok_or("mcp.choose_scope")?;
                    let id = scope.connection.ok_or("mcp.choose_scope")?;
                    let db = this.databases.as_ref().and_then(|db| db.upgrade()).ok_or("mcp.choose_scope")?;
                    let state = db.read(cx);
                    if state.note_connection_state(id) != Some(true) || scope.database.is_empty() { return Err("mcp.choose_scope"); }
                    let kind = state.note_profile(id).ok_or("mcp.choose_scope")?.kind;
                    let epoch = state.note_generation(id);
                    if let Some(sql) = &call.sql {
                            if !this.busy || this.stopping { return Err("mcp.stale"); }
                        notebook_query::command_required(kind, sql).ok_or("mcp.unsupported")?;
                    }
                    Ok((scope, id, kind, epoch, db, this.started))
                }).ok().and_then(Result::ok);
                let Some((scope, id, kind, epoch, db, turn)) = setup else {
                    let _ = call.response.try_send(Err("Select a connected database and check Settings > Database > SQL command permissions.".into())); continue;
                };
                let Some(sql) = call.sql else {
                    let allowed = view.update(cx, |this, cx| notebook_query::COMMANDS.iter().filter(|(key, _, _)| this.effective_sql_mode(cx).allows(key) && cx.global::<Preferences>().database.command_enabled(key)).map(|(_, label, _)| *label).collect::<Vec<_>>()).unwrap_or_default();
                    let _ = call.response.try_send(Ok(serde_json::json!({"connection_ref":id,"database":scope.database,"kind":kind.label(),"allowed_commands":allowed,"approval":"Confirm each SQL request in the TurboDBNote chat card. The application enforces the selected access mode and Settings SQL command permissions."}))); continue;
                };
                let Ok(prepared) = db.update(cx, |db, cx| db.mcp_request(id, epoch, scope.database.clone(), Action::NotePrepare { sql: sql.clone() }, cx)) else { break; };
                let Ok(Ok(Reply::NotePrepared(plan))) = prepared.recv().await else {
                    let _ = call.response.try_send(Err("SQL preparation failed. Check the selected database, supported syntax, and server permissions.".into())); continue;
                };
                if call.response.is_closed() { continue; }
                let key = notebook_query::command_required(kind, &plan.sql).unwrap_or("unsupported");
                let review_id = uuid::Uuid::new_v4();
                let installed = view.update(cx, |this, cx| {
                    if this.generation != generation || this.sql_scope.as_ref() != Some(&scope) || this.started != turn || !this.busy || this.stopping { return false; }
                    while this.sql_reviews.len() >= 8 { this.sql_reviews.remove(0); }
                    this.sql_reviews.push(SqlReview {
                        id: review_id, anchor: this.messages.len(), scope: scope.clone(), connection_generation: epoch,
                        chat_generation: generation, turn, command: key, sql: plan.sql.clone(), risky: !plan.read_only || plan.confirm,
                        state: ReviewState::Waiting, deadline: Instant::now() + Duration::from_secs(150), expanded: false, result: None, elapsed_ms: 0, rows: 0,
                    });
                    cx.notify(); true
                }).unwrap_or(false);
                if !installed { let _=call.response.try_send(Err("Request context changed".into())); continue; }
                let accepted = loop {
                    let decision = view.update(cx, |this, cx| {
                        let Some(index) = this.sql_reviews.iter().position(|card| card.id == review_id) else { return Some(false); };
                        let card = &this.sql_reviews[index];
                        if !this.review_current(card, cx) || call.response.is_closed() {
                            this.sql_reviews[index].state = ReviewState::Expired; cx.notify(); return Some(false);
                        }
                        match card.state {
                            ReviewState::Approved => {
                                if !this.effective_sql_mode(cx).allows(key) || !cx.global::<Preferences>().database.command_enabled(key) {
                                    this.sql_reviews[index].state = ReviewState::Waiting; cx.notify(); return None;
                                }
                                this.sql_reviews[index].state = ReviewState::Running; cx.notify(); Some(true)
                            }
                            ReviewState::Cancelled | ReviewState::Expired => Some(false),
                            _ => None,
                        }
                    }).unwrap_or(Some(false));
                    if let Some(accepted) = decision { break accepted; }
                    gpui::Timer::after(Duration::from_millis(100)).await;
                };
                if !accepted { let _ = call.response.try_send(Err("Request cancelled or context changed; no SQL was executed.".into())); continue; }
                let submitted = view.update(cx, |this, cx| {
                    let valid = this.sql_reviews.iter().find(|card| card.id == review_id).is_some_and(|card| this.review_current(card, cx))
                        && this.effective_sql_mode(cx).allows(key) && cx.global::<Preferences>().database.command_enabled(key) && !call.response.is_closed();
                    if !valid { if let Some(card)=this.sql_reviews.iter_mut().find(|c|c.id==review_id) { card.state=ReviewState::Expired; } cx.notify(); return None; }
                    Some(db.update(cx, |db, cx| db.mcp_request(id, epoch, scope.database.clone(), Action::NoteExecute { plan, confirmed: true, offset: 0, page_size: 20 }, cx)))
                }).ok().flatten();
                let Some(executed) = submitted else { let _=call.response.try_send(Err("Request expired before submission".into())); continue; };
                let run_started = Instant::now();
                let result = match executed.recv().await {
                    Ok(Ok(Reply::NoteResult(page))) => Ok(sql_mcp::result_data(page)),
                    _ => Err("Database operation failed or was cancelled. Writes may have taken effect; do not retry automatically.".into()),
                };
                let deliver = view.update(cx, |this, cx| {
                    let current = this.generation == generation && this.sql_scope.as_ref() == Some(&scope) && this.started == turn && !this.stopping;
                    if let Some(card)=this.sql_reviews.iter_mut().find(|card| card.id == review_id) {
                        card.elapsed_ms = run_started.elapsed().as_millis() as u64;
                        card.state = if result.is_ok() { ReviewState::Done } else { ReviewState::Failed };
                        if let Ok(data) = &result {
                            card.rows = data["rows"].as_array().map_or(0, Vec::len);
                            card.result = serde_json::to_string_pretty(data).ok();
                        }
                    }
                    cx.notify(); current
                }).unwrap_or(false);
                let _ = call.response.try_send(if deliver {result} else {Err("Request scope changed after submission; inspect the operation result in TurboDBNote. Do not automatically retry writes.".into())});
            }
        }));
        Some(config)
    }
}
