//! Reuse the existing search inputs and navigation store; filters never grant access.
use super::*;
use crate::catalog_cache::DatabaseRule;
use gpui_component::{WindowExt, checkbox::Checkbox};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum SearchScope {
    Connections,
    #[default]
    Relations,
    Programs,
}
impl SearchScope {
    fn hint(self) -> &'static str {
        match self {
            Self::Connections => "database.search.connection_hint",
            Self::Relations => "database.tree.filter",
            Self::Programs => "database.search.program_hint",
        }
    }
}
impl DatabaseView {
    pub(super) fn tree_object_query(&self, cx: &App) -> String {
        if self.tree.search_scope == SearchScope::Connections {
            String::new()
        } else {
            self.tree_filter.read(cx).value().trim().to_lowercase()
        }
    }
    pub(super) fn tree_connection_query(&self, cx: &App) -> String {
        if self.tree.search_scope == SearchScope::Connections {
            self.tree_filter.read(cx).value().trim().to_lowercase()
        } else {
            String::new()
        }
    }
    pub(super) fn tree_database_rule(&self, id: Uuid) -> DatabaseRule {
        self.navigation
            .database_rules
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn tree_search_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = cx.entity().downgrade();
        let selected = self.tree.search_scope;
        div()
            .h_flex()
            .gap_1()
            .min_w_0()
            .child(
                Input::new(&self.tree_filter)
                    .cleanable(true)
                    .flex_1()
                    .min_w_0(),
            )
            .child(
                Button::new("tree-search-scope")
                    .ghost()
                    .icon(gpui_component::Icon::default().path("icons/database-filter.svg"))
                    .dropdown_caret(false)
                    .tooltip(t(cx, selected.hint()))
                    .dropdown_menu(move |mut menu, _, cx| {
                        for (value, key) in [
                            (SearchScope::Connections, "database.search.connections"),
                            (SearchScope::Relations, "database.search.objects"),
                            (SearchScope::Programs, "database.search.programs"),
                        ] {
                            let owner = owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(t(cx, key))
                                    .checked(selected == value)
                                    .icon(IconName::Search)
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.tree.search_scope = value;
                                            this.tree_filter.update(cx, |input, cx| {
                                                input.set_placeholder(
                                                    t(cx, value.hint()),
                                                    window,
                                                    cx,
                                                );
                                                input.focus(window, cx);
                                            });
                                            this.tree.scroll.scroll_to_item(0, ScrollStrategy::Top);
                                            this.tree.revision += 1;
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }
    pub(in crate::database) fn database_rule_dialog(
        &self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.note_profile(id) else {
            return;
        };
        let name = profile.name.clone();
        let rule = self.tree_database_rule(id);
        let enabled = Rc::new(Cell::new(rule.enabled));
        let include = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(rule.include.join("\n"))
                .placeholder(t(cx, "database.rule.include_hint"))
        });
        let exclude = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(rule.exclude.join("\n"))
                .placeholder(t(cx, "database.rule.exclude_hint"))
        });
        let owner = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let toggle = enabled.clone();
            let saved = enabled.clone();
            let include = include.clone();
            let exclude = exclude.clone();
            let owner = owner.clone();
            dialog
                .title(t(cx, "database.rule.title"))
                .w(px(560.))
                .overlay_closable(false)
                .child(
                    div()
                        .v_flex()
                        .gap_3()
                        .child(div().text_sm().truncate().child(name.clone()))
                        .child(
                            Checkbox::new("database-rule-enabled")
                                .label(t(cx, "database.rule.enabled"))
                                .checked(enabled.get())
                                .on_click(move |checked, _, cx| {
                                    toggle.set(*checked);
                                    cx.refresh_windows();
                                }),
                        )
                        .child(t(cx, "database.rule.include"))
                        .child(Input::new(&include).cleanable(true).h(px(96.)))
                        .child(t(cx, "database.rule.exclude"))
                        .child(Input::new(&exclude).cleanable(true).h(px(96.)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(cx, "database.rule.hint")),
                        )
                        .child(
                            feedback::actions().child(feedback::close_button(cx)).child(
                                Button::new("apply-database-rule")
                                    .primary()
                                    .label(t(cx, "database.rule.apply"))
                                    .on_click(move |_, window, cx| {
                                        let patterns = |input: &Entity<InputState>, cx: &App| {
                                            input
                                                .read(cx)
                                                .value()
                                                .lines()
                                                .map(str::trim)
                                                .filter(|s| !s.is_empty())
                                                .map(str::to_owned)
                                                .collect::<Vec<_>>()
                                        };
                                        let rule = DatabaseRule {
                                            enabled: saved.get(),
                                            include: patterns(&include, cx),
                                            exclude: patterns(&exclude, cx),
                                        };
                                        let applied = owner
                                            .update(cx, |this, cx| {
                                                this.apply_database_rule(id, rule, cx)
                                            })
                                            .unwrap_or(false);
                                        if applied {
                                            window.close_dialog(cx);
                                            feedback::success(t(cx, "database.rule.applied"), cx);
                                        }
                                    }),
                            ),
                        ),
                )
        });
    }
    fn apply_database_rule(
        &mut self,
        id: Uuid,
        rule: DatabaseRule,
        cx: &mut Context<Self>,
    ) -> bool {
        if !rule.valid() {
            feedback::error(t(cx, "database.rule.invalid"), cx);
            return false;
        }
        if !self.profiles.iter().any(|p| p.id == id) {
            feedback::error(t(cx, "database.rule.missing_connection"), cx);
            return false;
        }
        let mut navigation = self.navigation.clone();
        if rule == DatabaseRule::default() {
            navigation.database_rules.remove(&id);
        } else {
            navigation.database_rules.insert(id, rule);
        }
        if !serde_json::to_vec(&navigation).is_ok_and(|bytes| bytes.len() <= 65536) {
            feedback::error(t(cx, "database.rule.storage_limit"), cx);
            return false;
        }
        self.navigation = navigation;
        if let Some(page) = self.tree.databases.get_mut(&id) {
            page.error = None;
        }
        self.tree.catalog_revision += 1;
        self.tree.revision += 1;
        self.persist_navigation();
        cx.notify();
        true
    }
    pub(in crate::database) fn toggle_database_rule(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let mut rule = self.tree_database_rule(id);
        rule.enabled = !rule.enabled;
        self.apply_database_rule(id, rule, cx);
    }
}
