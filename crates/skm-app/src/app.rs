use std::{collections::BTreeMap, sync::mpsc};

use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::*, px,
    uniform_list,
};
use skm_core::{engine::Operation, projects::Project};

use crate::{
    backend::{self, CoreThread, Shutdown, Update},
    model::{Item, ItemChanges, Model},
    theme::{PATH_FONT, TEXT_FONT, Theme},
};

#[derive(Clone, PartialEq, Eq)]
enum Section {
    Kind(&'static str, &'static str),
    Project(i64),
    Sets,
    Activity,
}

struct ItemRow(Item);

impl Render for ItemRow {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        div()
            .flex()
            .items_center()
            .h(px(32.))
            .px(px(16.))
            .gap(px(24.))
            .border_b_1()
            .border_color(theme.line)
            .child(
                div()
                    .w(px(240.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(self.0.name.clone()),
            )
            .child(
                div()
                    .font_family(PATH_FONT)
                    .text_color(theme.muted)
                    .overflow_hidden()
                    .child(self.0.path.clone()),
            )
    }
}

pub struct Skillsmgr {
    model: Model,
    rows: BTreeMap<i64, Entity<ItemRow>>,
    projects: Vec<Project>,
    sets: Vec<String>,
    activity: Vec<Operation>,
    section: Section,
    error: Option<String>,
    _shutdown: Shutdown,
    _bridge: Task<()>,
    _appearance: Subscription,
}

impl Skillsmgr {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let guard = CoreThread::new();
        let (shutdown, receiver, events) = backend::channel();
        let stopped = shutdown.stopped();
        let manager_home = std::env::var_os("SKILLSMGR_HOME").map(Into::into);
        let user_home = std::env::var_os("HOME").map(Into::into);
        let (updates, incoming) = mpsc::channel();
        cx.background_executor()
            .spawn(async move {
                backend::run(
                    guard,
                    stopped,
                    events,
                    receiver,
                    updates,
                    manager_home,
                    user_home,
                );
            })
            .detach();
        let bridge = cx.spawn(async move |this, cx| {
            let mut incoming = incoming;
            loop {
                let (receiver, message) = cx
                    .background_executor()
                    .spawn(async move {
                        let message = incoming.recv();
                        (incoming, message)
                    })
                    .await;
                incoming = receiver;
                let Ok(message) = message else {
                    break;
                };
                if this
                    .update(cx, |this, cx| this.receive(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let appearance = cx.observe_window_appearance(window, |this, _, cx| {
            for row in this.rows.values() {
                row.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        });
        Self {
            model: Model::default(),
            rows: BTreeMap::new(),
            projects: vec![],
            sets: vec![],
            activity: vec![],
            section: Section::Kind("skill", "Skills"),
            error: None,
            _shutdown: shutdown,
            _bridge: bridge,
            _appearance: appearance,
        }
    }

    fn receive(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Loaded(snapshot) => {
                self.projects = snapshot.projects;
                self.sets = snapshot.sets;
                self.activity = snapshot.activity;
                self.update_rows(
                    ItemChanges {
                        ids: snapshot.items.iter().map(|row| row.id).collect(),
                        rows: snapshot.items,
                    },
                    cx,
                );
                cx.notify();
            }
            Update::Items(changes) => self.update_rows(changes, cx),
            Update::Error(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn update_rows(&mut self, changes: ItemChanges, cx: &mut Context<Self>) {
        let mut structure_changed = false;
        for id in self.model.apply(changes) {
            match (self.rows.get(&id), self.model.items.get(&id)) {
                (Some(entity), Some(item)) => {
                    entity.update(cx, |row, cx| {
                        structure_changed |= row.0.kind != item.kind;
                        row.0 = item.clone();
                        cx.notify();
                    });
                }
                (None, Some(item)) => {
                    self.rows.insert(id, cx.new(|_| ItemRow(item.clone())));
                    structure_changed = true;
                }
                (Some(_), None) => {
                    self.rows.remove(&id);
                    structure_changed = true;
                }
                (None, None) => {}
            }
        }
        if structure_changed {
            cx.notify();
        }
    }

    fn nav(
        &self,
        id: impl Into<SharedString>,
        label: String,
        section: Section,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.section == section;
        div()
            .id(id.into())
            .h(px(30.))
            .px(px(10.))
            .mx(px(8.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .bg(if selected {
                theme.selected
            } else {
                theme.sidebar
            })
            .text_color(if selected {
                theme.accent
            } else {
                theme.foreground
            })
            .cursor_pointer()
            .hover(move |style| style.bg(theme.selected))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.section = section.clone();
                cx.notify();
            }))
            .child(label)
    }
}

impl Render for Skillsmgr {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        let mut sidebar = div()
            .id("sidebar")
            .w(px(220.))
            .h_full()
            .overflow_y_scroll()
            .flex_shrink_0()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.line)
            .py(px(12.));
        for (kind, label) in [
            ("skill", "Skills"),
            ("mcp", "MCP"),
            ("hook", "Hooks"),
            ("subagent", "Subagents"),
            ("plugin", "Plugins"),
        ] {
            let count = self
                .model
                .items
                .values()
                .filter(|item| item.kind == kind)
                .count();
            sidebar = sidebar.child(self.nav(
                kind,
                format!("{label}  {count}"),
                Section::Kind(kind, label),
                theme,
                cx,
            ));
        }
        sidebar = sidebar.child(
            div()
                .mt(px(20.))
                .mb(px(4.))
                .px(px(18.))
                .text_color(theme.muted)
                .child("Projects"),
        );
        for project in &self.projects {
            sidebar = sidebar.child(self.nav(
                format!("project-{}", project.id),
                project.name.clone(),
                Section::Project(project.id),
                theme,
                cx,
            ));
        }
        sidebar = sidebar
            .child(div().h(px(20.)))
            .child(self.nav("sets", "Sets".into(), Section::Sets, theme, cx))
            .child(self.nav("activity", "Activity".into(), Section::Activity, theme, cx));

        let title = match &self.section {
            Section::Kind(_, label) => label.to_string(),
            Section::Project(id) => self
                .projects
                .iter()
                .find(|p| p.id == *id)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            Section::Sets => "Sets".into(),
            Section::Activity => "Activity".into(),
        };
        let mut content = div().flex().flex_col().flex_1().min_w_0().h_full().child(
            div()
                .flex()
                .items_center()
                .h(px(48.))
                .flex_shrink_0()
                .px(px(16.))
                .border_b_1()
                .border_color(theme.line)
                .child(title),
        );
        if let Some(error) = &self.error {
            content = content.child(div().px(px(16.)).py(px(8.)).child(error.clone()));
        }
        content = match &self.section {
            Section::Kind(kind, _) => {
                let rows: Vec<_> = self
                    .model
                    .items
                    .values()
                    .filter(|item| item.kind == *kind)
                    .filter_map(|item| self.rows.get(&item.id).cloned())
                    .collect();
                content.child(
                    uniform_list("items", rows.len(), move |range, _, _| {
                        range
                            .map(|i| {
                                rows[i].clone().cached(
                                    gpui_kit::StyleRefinement::default().w_full().h(px(32.)),
                                )
                            })
                            .collect()
                    })
                    .flex_1(),
                )
            }
            Section::Project(id) => {
                for project in self.projects.iter().filter(|p| p.id == *id) {
                    content = content.child(
                        div()
                            .p(px(16.))
                            .font_family(PATH_FONT)
                            .child(project.path.to_string_lossy().into_owned()),
                    );
                }
                content
            }
            Section::Sets => {
                for name in &self.sets {
                    content = content.child(div().px(px(16.)).py(px(8.)).child(name.clone()));
                }
                content
            }
            Section::Activity => {
                for operation in &self.activity {
                    content = content.child(
                        div()
                            .px(px(16.))
                            .py(px(8.))
                            .child(operation.summary.clone()),
                    );
                }
                content
            }
        };
        div()
            .size_full()
            .flex()
            .font_family(TEXT_FONT)
            .text_size(px(13.))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(sidebar)
            .child(content)
    }
}
