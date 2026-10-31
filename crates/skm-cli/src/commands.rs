use crate::{
    Context,
    args::{AgentCommand, Command, Kind},
    output,
};
use serde_json::{Value, json};
use skm_core::{
    Error,
    engine::{self, Action, Ownership, Plan, PlannedAction},
    library::Library,
    target::{ItemKind, ItemRef, TargetClass},
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn run(c: &Context) -> Result<i32, Error> {
    match c.cli.command.as_ref().unwrap_or(&Command::Status) {
        Command::Status => {
            let items = skm_core::indexer::load(&c.store)?;
            let projects = skm_core::projects::list(&c.store)?;
            let enabled = enabled(c)?;
            let pending = projects
                .iter()
                .map(|p| skm_core::projects::sync::pending_count(&c.store, p.id))
                .collect::<Result<Vec<_>, _>>()?;
            let value = json!({"items":items.len(),"agents":enabled,"projects":projects,"pending":pending.iter().sum::<usize>(),"pending_updates":0});
            output::data(c, &value, || {
                println!(
                    "{} items, {} projects, {} pending\n{} enabled agents",
                    items.len(),
                    projects.len(),
                    pending.iter().sum::<usize>(),
                    value["agents"]
                )
            })
        }
        Command::List {
            kind,
            scope,
            foreign,
        } => list(c, *kind, scope.as_deref(), *foreign),
        Command::Show { item } => {
            let item = item_ref(c, item)?;
            let row = skm_core::indexer::load(&c.store)?
                .into_iter()
                .find(|row| row.kind == kind_name(item.kind) && row.name == item.name)
                .ok_or_else(|| Error::InvalidSkill(format!("unknown item {:?}", item.name)))?;
            let active = active(c, &item)?;
            let value = json!({"kind":row.kind,"name":row.name,"description":row.description,"path":c.paths.library.join(row.rel_path),"metadata":parse_json(&row.meta_json)?,"source":row.source_json.as_deref().map(parse_json).transpose()?,"active":active,"problems":parse_json(&row.problems_json)?});
            output::data(c, &value, || {
                println!("{} {}\n{}\n{}", row.kind, row.name, row.description, value)
            })
        }
        Command::Search { query, limit } => {
            let results = skm_core::search::SearchClient::from_env().search(query, *limit)?;
            let value: Vec<_> = results.iter().map(|r| json!({"name":r.name,"source":r.source,"install_source":r.install_source(),"installs":r.installs})).collect();
            output::data(c, &value, || {
                for r in &results {
                    println!("{}\t{}\t{}", r.name, r.install_source(), r.installs);
                }
            })
        }
        Command::On { item, scope, agent } | Command::Off { item, scope, agent } => toggle(
            c,
            item,
            scope,
            agent,
            matches!(c.cli.command, Some(Command::On { .. })),
        ),
        Command::Add {
            source,
            skill,
            kind,
            path,
            on,
            set,
        } => add(
            c,
            source,
            skill,
            *kind,
            path.as_deref(),
            on.as_deref(),
            set.as_deref(),
        ),
        Command::Remove { item, off } => remove(c, item, *off),
        Command::Foreign { project } => foreign(c, project.as_deref()),
        Command::Adopt {
            names,
            all,
            scope,
            project,
        } => {
            let project = project.clone().or_else(|| {
                scope
                    .as_ref()
                    .filter(|s| s.as_str() != "global")
                    .map(PathBuf::from)
            });
            let names = if *all {
                skm_core::foreign_skills::list_foreign(
                    &c.paths,
                    &c.store,
                    &c.home,
                    project.as_deref(),
                )?
                .into_iter()
                .map(|s| s.name)
                .collect()
            } else {
                names
                    .iter()
                    .map(|name| {
                        let path = Path::new(name);
                        if path.components().count() > 1 {
                            let root = project.as_deref().unwrap_or(&c.home).canonicalize()?.join(".agents/skills");
                            let parent = std::path::absolute(path)?.parent().ok_or_else(||Error::InvalidSkill("adopt needs a foreign skill path".into()))?.canonicalize()?;
                            if parent != root.canonicalize()? { return Err(Error::ForeignTarget("adopt path must be in the selected scope's .agents/skills directory".into())); }
                        }
                        Ok(path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(name)
                            .to_owned())
                    })
                    .collect::<Result<Vec<_>,Error>>()?
            };
            if c.cli.dry_run {
                let plan = skm_core::foreign_skills::plan_adopt(
                    &c.paths,
                    &c.store,
                    &c.home,
                    &names,
                    project.as_deref(),
                )?;
                return output::plan(c, Value::Null, &[plan]);
            }
            let result = skm_core::foreign_skills::adopt(
                &c.paths,
                &c.store,
                &c.home,
                &names,
                project.as_deref(),
            )?;
            output::plan(c, Value::Null, &[recorded_plan(c, result.op_id)?])?;
            let code = output::applied(c, &result);
            skm_core::indexer::catch_up(&c.store, &c.paths, None)?;
            Ok(code)
        }
        Command::Set { command } => crate::sets::run(c, command),
        Command::Project { command } => crate::project::run(c, command),
        Command::Agents { .. } => agents(c),
        Command::Agent { command } => {
            let (id, on) = match command {
                AgentCommand::Enable { id } => (id, true),
                AgentCommand::Disable { id } => (id, false),
            };
            if !c.registry.agents.contains_key(id) {
                return Err(Error::ConfigParse(format!("unknown agent {id:?}")));
            }
            output::plan(c, json!({"agent":id,"enabled":on}), &[])?;
            if !c.cli.dry_run {
                let id = id.clone();
                c.store.write(move |db| { db.execute("INSERT INTO agents(id,detected,enabled) VALUES(?1,0,?2) ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled", (&id,on))?; Ok(()) })?;
                changed(c)?;
            }
            Ok(0)
        }
        Command::Undo { op_id, list } => {
            if *list {
                let ops = engine::list_ops(&c.store)?;
                let value: Vec<_> = ops.iter().map(|op| json!({"id":op.id,"at":op.at,"actor":op.actor,"result":op.result,"summary":op.summary})).collect();
                return output::data(c, &value, || {
                    println!("ID\tAT\tACTOR\tRESULT\tSUMMARY");
                    for op in ops {
                        println!(
                            "{}\t{}\t{}\t{}\t{}",
                            op.id, op.at, op.actor, op.result, op.summary
                        );
                    }
                });
            }
            if c.cli.dry_run {
                let plan = engine::plan_undo(&c.paths, &c.store, *op_id)?;
                let count = plan.actions.len();
                let code = output::plan(c, Value::Null, &[plan])?;
                if !c.cli.json && !c.cli.quiet {
                    println!("{count} undone");
                }
                return Ok(code);
            }
            let result = engine::undo(&c.paths, &c.store, *op_id, "cli", None)?;
            output::plan(c, Value::Null, &[recorded_plan(c, result.op_id)?])?;
            if !c.cli.json && !c.cli.quiet {
                println!("{} undone", result.done.len());
            }
            let code = output::applied(c, &result);
            skm_core::indexer::catch_up(&c.store, &c.paths, None)?;
            Ok(code)
        }
        Command::Doctor { .. } => unreachable!(),
    }
}

fn parse_json(text: &str) -> Result<Value, Error> {
    serde_json::from_str(text).map_err(|e| Error::ConfigParse(e.to_string()))
}

pub fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Skill => "skill",
        ItemKind::Mcp => "mcp",
        ItemKind::Hook => "hook",
        ItemKind::Subagent => "subagent",
        ItemKind::Plugin => "plugin",
    }
}

pub fn item_ref(c: &Context, text: &str) -> Result<ItemRef, Error> {
    let (kind, name) = if let Some((kind, name)) = text.split_once(':') {
        let kind = match kind {
            "skill" => ItemKind::Skill,
            "mcp" => ItemKind::Mcp,
            "hook" => ItemKind::Hook,
            "subagent" => ItemKind::Subagent,
            "plugin" => ItemKind::Plugin,
            _ => return Err(Error::ConfigParse(format!("unknown item kind {kind:?}"))),
        };
        (kind, name.to_owned())
    } else {
        let matches: Vec<_> = skm_core::indexer::load(&c.store)?
            .into_iter()
            .filter(|r| r.name == text)
            .collect();
        if matches.len() > 1 {
            return Err(Error::ConfigParse("ambiguous item; use kind:name".into()));
        }
        if let Some(row) = matches.first() {
            return item_ref(c, &format!("{}:{}", row.kind, row.name));
        }
        (ItemKind::Skill, text.to_owned())
    };
    skm_core::global_skills::validate_name(&name)?;
    Ok(ItemRef { kind, name })
}

pub fn global_scope(c: &Context, create: bool) -> Result<i64, Error> {
    if create {
        c.store.write(|db| { db.execute("INSERT INTO scopes(kind,name,mode,added_at) SELECT 'global','Global','link',unixepoch() WHERE NOT EXISTS(SELECT 1 FROM scopes WHERE kind='global')", [])?; Ok(()) })?;
    }
    c.store.read(|db| {
        Ok(db.query_row(
            "SELECT COALESCE((SELECT id FROM scopes WHERE kind='global' ORDER BY id LIMIT 1),0)",
            [],
            |r| r.get(0),
        )?)
    })
}

fn recorded_plan(c: &Context, op_id: i64) -> Result<Plan, Error> {
    let text: String = c.store.read(|db| {
        Ok(
            db.query_row("SELECT plan_json FROM ops WHERE id=?1", [op_id], |row| {
                row.get(0)
            })?,
        )
    })?;
    serde_json::from_str(&text).map_err(|error| Error::ConfigParse(error.to_string()))
}

pub fn changed(c: &Context) -> Result<(), Error> {
    fs::write(c.paths.home.join(".changed"), [])?;
    Ok(())
}

pub fn pending_set(c: &Context, name: &str) -> Result<(), Error> {
    let projects: Vec<_> = skm_core::projects::projects_using_set(&c.store, name)?
        .into_iter()
        .filter(|p| !p.auto_sync)
        .map(|p| p.id)
        .collect();
    let key = format!("set:{name}");
    c.store.write(move |db| {
        let transaction = db.transaction()?;
        for id in projects {
            transaction.execute("INSERT INTO project_pending(scope_id,change_key) VALUES(?1,?2) ON CONFLICT(scope_id,change_key) DO UPDATE SET revision=revision+1",(id,&key))?;
        }
        transaction.commit()?;
        Ok(())
    })
}

pub fn manifest_plan(c: &Context, library: &Library) -> Result<Plan, Error> {
    let target = c.paths.library.join("library.toml");
    let existing = match fs::read_to_string(&target) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let bytes = library.render(&existing, &c.known_agents())?.into_bytes();
    let before = engine::observe(&c.paths, &c.store, std::slice::from_ref(&target))?.remove(0);
    if !before.parents_safe
        || !matches!(
            before.state.class,
            TargetClass::Absent | TargetClass::RealFile
        )
    {
        return Err(Error::ForeignTarget("unsafe library manifest".into()));
    }
    let scope_id = global_scope(c, false)?;
    let mut action = PlannedAction {
        action: Action::WriteFile { target, bytes },
        before,
        owner: Some(Ownership {
            scope_id,
            agent_id: None,
            item: ItemRef {
                kind: ItemKind::Skill,
                name: "library.toml".into(),
            },
            how: "file".into(),
            written_hash: String::new(),
        }),
        approve_modified: false,
        adoption: None,
        requires: vec![],
    };
    if action.before.state.class != TargetClass::Absent {
        action.authorize_adoption();
    }
    Ok(Plan {
        scope_id,
        actions: vec![action],
        issues: vec![],
        summary: "Save library".into(),
    })
}

pub fn bind_global(c: &Context, plan: &mut Plan) -> Result<(), Error> {
    let id = global_scope(c, true)?;
    plan.scope_id = id;
    for action in &mut plan.actions {
        if let Some(owner) = &mut action.owner {
            owner.scope_id = id;
        }
    }
    Ok(())
}

fn enabled(c: &Context) -> Result<BTreeSet<String>, Error> {
    c.store.read(|db| {
        let mut q = db.prepare("SELECT id FROM agents WHERE enabled=1 ORDER BY id")?;
        Ok(q.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
    })
}

fn agents(c: &Context) -> Result<i32, Error> {
    let config = skm_core::config::Config::load(&c.paths.config_file)?;
    let registry = c.registry.resolve(
        &c.home,
        &std::env::vars_os()
            .filter_map(|(k, v)| k.into_string().ok().map(|k| (k, v)))
            .collect(),
        &config,
    )?;
    let statuses = registry.detect(&std::env::var_os("PATH").unwrap_or_default(), &enabled(c)?)?;
    if !c.cli.dry_run {
        let statuses = statuses.clone();
        c.store.write(move |db| { let tx = db.transaction()?; for s in statuses { tx.execute("INSERT INTO agents(id,detected,enabled,detected_at) VALUES(?1,?2,0,unixepoch()) ON CONFLICT(id) DO UPDATE SET detected=excluded.detected,detected_at=excluded.detected_at", (&s.id,s.detected))?; } tx.commit()?; Ok(()) })?;
    }
    let value: Vec<_> = statuses
        .iter()
        .map(|s| json!({"id":s.id,"name":s.name,"detected":s.detected,"enabled":s.enabled}))
        .collect();
    output::data(c, &value, || {
        println!("{:<14} {:<20} {:<10} ENABLED", "ID", "AGENT", "DETECTED");
        for s in statuses {
            println!(
                "{:<14} {:<20} {:<10} {}",
                s.id, s.name, s.detected, s.enabled
            );
        }
    })
}

fn foreign(c: &Context, project: Option<&Path>) -> Result<i32, Error> {
    let entries = skm_core::foreign_skills::list_foreign(&c.paths, &c.store, &c.home, project)?;
    let value: Vec<_> = entries
        .iter()
        .map(|s| json!({"name":s.name,"class":s.class,"provenance":s.provenance}))
        .collect();
    output::data(c, &value, || {
        for s in entries {
            println!("{}\t{:?}\t{:?}", s.name, s.class, s.provenance);
        }
    })
}

fn list(
    c: &Context,
    kind: Option<Kind>,
    scope: Option<&str>,
    unmanaged: bool,
) -> Result<i32, Error> {
    if unmanaged {
        return foreign(c, scope.filter(|s| *s != "global").map(Path::new));
    }
    let selection = match scope {
        Some("global") => Some(c.library()?.global),
        Some(path) => {
            let p = crate::project::find(c, Path::new(path))?;
            Some(skm_core::projects::effective_project(
                &c.store,
                &c.library()?,
                p.id,
                &c.registry,
            )?)
        }
        None => None,
    };
    let rows: Vec<_> = skm_core::indexer::load(&c.store)?
        .into_iter()
        .filter(|r| kind.is_none_or(|k| k.name() == r.kind))
        .filter(|r| {
            selection.as_ref().is_none_or(|s| match r.kind.as_str() {
                "skill" => s.skills.contains(&r.name),
                "mcp" => s.mcp.contains_key(&r.name),
                "hook" => s.hooks.contains_key(&r.name),
                "subagent" => s.subagents.contains_key(&r.name),
                "plugin" => s.plugins.contains_key(&r.name),
                _ => false,
            })
        })
        .collect();
    let value: Vec<_> = rows
        .iter()
        .map(|r| json!({"kind":r.kind,"name":r.name,"description":r.description}))
        .collect();
    output::data(c, &value, || {
        println!("KIND\tNAME\tDESCRIPTION");
        for r in rows {
            println!("{}\t{}\t{}", r.kind, r.name, r.description);
        }
    })
}

fn active(c: &Context, item: &ItemRef) -> Result<Vec<String>, Error> {
    let library = c.library()?;
    let contains = |s: &skm_core::library::ItemSelection| {
        if item.kind == ItemKind::Skill {
            s.skills.contains(&item.name)
        } else {
            s.entries(item.kind)
                .is_some_and(|e| e.contains_key(&item.name))
        }
    };
    let mut scopes = Vec::new();
    if contains(&library.global) {
        scopes.push("global".into());
    }
    for p in skm_core::projects::list(&c.store)? {
        if contains(&skm_core::projects::effective_project(
            &c.store,
            &library,
            p.id,
            &c.registry,
        )?) {
            scopes.push(p.path.to_string_lossy().into_owned());
        }
    }
    Ok(scopes)
}

fn toggle(c: &Context, text: &str, scope: &str, agents: &[String], on: bool) -> Result<i32, Error> {
    let item = item_ref(c, text)?;
    if item.kind == ItemKind::Skill && !agents.is_empty() {
        return Err(Error::ConfigParse(
            "skills are scoped, not agent-specific".into(),
        ));
    }
    if scope != "global" {
        return crate::project::toggle(c, &item, Path::new(scope), agents, on);
    }
    if item.kind != ItemKind::Skill {
        let mut library = c.library()?;
        let entries = match item.kind {
            ItemKind::Mcp => &mut library.global.mcp,
            ItemKind::Hook => &mut library.global.hooks,
            ItemKind::Subagent => &mut library.global.subagents,
            ItemKind::Plugin => &mut library.global.plugins,
            ItemKind::Skill => unreachable!(),
        };
        if on {
            entries.insert(
                item.name.clone(),
                if agents.is_empty() {
                    vec!["*".into()]
                } else {
                    agents.to_vec()
                },
            );
        } else {
            entries.remove(&item.name);
        }
        let mut plan = manifest_plan(c, &library)?;
        let code = output::plan(
            c,
            json!({"scope":"global","item":item,"on":on,"agents":agents}),
            std::slice::from_ref(&plan),
        )?;
        if c.cli.dry_run {
            return Ok(code);
        }
        bind_global(c, &mut plan)?;
        return output::apply(c, &[plan]);
    }
    if c.cli.dry_run {
        let plan = skm_core::global_skills::plan_global_skill(
            &c.paths, &c.store, &c.home, &item.name, on,
        )?;
        return output::plan(c, json!({"scope":"global","item":item,"on":on}), &[plan]);
    }
    let result =
        skm_core::global_skills::set_global_skill(&c.paths, &c.store, &c.home, &item.name, on)?;
    output::plan(
        c,
        json!({"scope":"global","item":item,"on":on}),
        &[recorded_plan(c, result.op_id)?],
    )?;
    Ok(output::applied(c, &result))
}

#[allow(clippy::too_many_arguments)]
fn add(
    c: &Context,
    source: &str,
    names: &[String],
    kind: Kind,
    path: Option<&str>,
    on: Option<&str>,
    set: Option<&str>,
) -> Result<i32, Error> {
    if !matches!(kind, Kind::Skill) {
        return Err(Error::ConfigParse(
            "only skill installation is available in M1".into(),
        ));
    }
    let source = skm_core::source::parse(source, path)?;
    let git = skm_core::git::GitRunner::new(Duration::from_secs(30))?;
    let preview = skm_core::install::preview(&c.paths, &git, &source)?;
    for name in names {
        if !preview.skills().iter().any(|s| &s.name == name) {
            return Err(Error::InvalidSkill(format!("source has no skill {name:?}")));
        }
    }
    let selections: Vec<_> = preview
        .skills()
        .iter()
        .enumerate()
        .filter(|(_, s)| names.is_empty() || names.contains(&s.name))
        .map(|(skill, _)| skm_core::install::Selection {
            skill,
            collision: Default::default(),
        })
        .collect();
    if let Some(name) = set
        && !c.library()?.sets.contains_key(name)
    {
        return Err(Error::ConfigParse(format!("unknown set {name:?}")));
    }
    if let Some(scope) = on
        && scope != "global"
    {
        crate::project::find(c, Path::new(scope))?;
    }
    let mut prepared = skm_core::install::plan_install(&c.paths, &c.store, &preview, &selections)?
        .map_err(|collisions| {
            Error::NameCollision(
                collisions
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        })?;
    let installed: Vec<_> = prepared.items.iter().map(|s| s.name.clone()).collect();
    let sources: std::collections::BTreeMap<_, _> = prepared
        .plan
        .actions
        .iter()
        .filter_map(|action| {
            if let Action::CopyTree { target, from, .. } = &action.action
                && target.parent() == Some(c.paths.library.join("skills").as_path())
            {
                return Some((target.file_name()?.to_str()?.to_owned(), from.clone()));
            }
            None
        })
        .collect();
    let manifest = c.paths.library.join("library.toml");
    let write = prepared
        .plan
        .actions
        .iter_mut()
        .find(|a| a.action.target() == manifest)
        .unwrap();
    let Action::WriteFile { bytes, .. } = &mut write.action else {
        unreachable!()
    };
    let text = String::from_utf8(bytes.clone()).map_err(|e| Error::ConfigParse(e.to_string()))?;
    let mut library = Library::parse(&text, &c.known_agents())?;
    for name in &installed {
        let item = ItemRef {
            kind: ItemKind::Skill,
            name: name.clone(),
        };
        if let Some(set) = set {
            library
                .sets
                .get_mut(set)
                .unwrap()
                .items
                .set_item(&item, None)?;
        }
        if on == Some("global") {
            library.global.set_item(&item, None)?;
        }
    }
    *bytes = library.render(&text, &c.known_agents())?.into_bytes();
    let mut followup = Vec::new();
    if on == Some("global") {
        let desired: Vec<_> = installed
            .iter()
            .map(|name| engine::DesiredTarget {
                target: c.home.join(".agents/skills").join(name),
                item: ItemRef {
                    kind: ItemKind::Skill,
                    name: name.clone(),
                },
                agent_id: None,
                content: engine::DesiredContent::Link {
                    to: c.paths.library.join("skills").join(name),
                },
                approve_modified: false,
            })
            .collect();
        let observations = engine::observe(
            &c.paths,
            &c.store,
            &desired.iter().map(|d| d.target.clone()).collect::<Vec<_>>(),
        )?;
        followup.push(engine::plan(
            global_scope(c, false)?,
            &desired,
            &observations,
        ));
    }
    let on_project = on
        .filter(|scope| *scope != "global")
        .map(|scope| crate::project::find(c, Path::new(scope)))
        .transpose()?;
    for project in skm_core::projects::list(&c.store)? {
        let sets = skm_core::projects::used_sets(&c.store, project.id)?;
        let selected = on_project.as_ref().is_some_and(|p| p.id == project.id);
        if !selected
            && !(project.auto_sync && set.is_some_and(|name| sets.iter().any(|s| s == name)))
        {
            continue;
        }
        let mut direct = crate::project::direct(c, &project)?;
        if selected {
            for name in &installed {
                direct.set_item(
                    &ItemRef {
                        kind: ItemKind::Skill,
                        name: name.clone(),
                    },
                    None,
                )?;
            }
        }
        let effective = crate::project::selection(c, &library, &project, &sets, &direct)?;
        followup.push(skm_core::projects::plan_staged_project(
            &c.paths, &c.store, &project, &library, &effective, &sources,
        )?);
    }
    let mut plans = vec![prepared.plan.clone()];
    plans.extend(followup.iter().cloned());
    let code = output::plan(c, json!({"install":installed,"on":on,"set":set}), &plans)?;
    if c.cli.dry_run {
        return Ok(code);
    }
    let result = prepared.apply(&c.paths, &c.store)?;
    let code = match result {
        skm_core::install::InstallResult::Applied { result, .. } => output::applied(c, &result),
        _ => unreachable!(),
    };
    skm_core::indexer::catch_up(&c.store, &c.paths, None)?;
    if code != 0 {
        return Ok(code);
    }
    if let Some(project) = on_project {
        for name in &installed {
            skm_core::projects::set_direct_item(
                &c.store,
                &c.registry,
                project.id,
                &ItemRef {
                    kind: ItemKind::Skill,
                    name: name.clone(),
                },
                None,
            )?;
        }
    }
    if on == Some("global") {
        bind_global(c, &mut followup[0])?;
    }
    let code = output::apply(c, &followup)?;
    if let Some(set) = set {
        pending_set(c, set)?;
    }
    Ok(code)
}

fn remove(c: &Context, text: &str, off: bool) -> Result<i32, Error> {
    let item = item_ref(c, text)?;
    let active = active(c, &item)?;
    if !active.is_empty() && !off {
        return Err(Error::ForeignTarget(
            "item is active; use --off to disable it before removal".into(),
        ));
    }
    let row = skm_core::indexer::load(&c.store)?
        .into_iter()
        .find(|r| r.kind == kind_name(item.kind) && r.name == item.name)
        .ok_or_else(|| Error::InvalidSkill(format!("unknown item {:?}", item.name)))?;
    let target = c.paths.library.join(row.rel_path);
    let before = engine::observe(&c.paths, &c.store, std::slice::from_ref(&target))?.remove(0);
    if !before.parents_safe {
        return Err(Error::ForeignTarget("unsafe library item".into()));
    }
    let mut action = PlannedAction {
        action: Action::Trash { target },
        before,
        owner: None,
        approve_modified: true,
        adoption: None,
        requires: vec![],
    };
    action.authorize_adoption();
    let mut library = c.library()?;
    library.skills.remove(&item.name);
    library.global.remove_item(&item);
    for set in library.sets.values_mut() {
        set.items.remove_item(&item);
    }
    let mut plan = manifest_plan(c, &library)?;
    plan.actions.insert(0, action);
    plan.summary = format!("Remove {}", item.name);
    let mut plans = Vec::new();
    if active.iter().any(|scope| scope == "global") && item.kind == ItemKind::Skill {
        plans.push(skm_core::global_skills::plan_global_skill(
            &c.paths, &c.store, &c.home, &item.name, false,
        )?);
    }
    let projects = skm_core::projects::list(&c.store)?;
    for project in &projects {
        if item.kind != ItemKind::Skill {
            continue;
        }
        let target = project.path.join(".agents/skills").join(&item.name);
        let observed = engine::observe(&c.paths, &c.store, &[target])?.remove(0);
        if !active.iter().any(|scope| Path::new(scope) == project.path)
            && !observed
                .ledger
                .is_some_and(|owner| owner.scope_id == project.id)
        {
            continue;
        }
        let mut direct = crate::project::direct(c, project)?;
        direct.remove_item(&item);
        let selected = crate::project::selection(
            c,
            &library,
            project,
            &skm_core::projects::used_sets(&c.store, project.id)?,
            &direct,
        )?;
        let names = BTreeSet::from([item.name.clone()]);
        plans.push(match project.mode {
            skm_core::projects::ProjectMode::Copy => skm_core::projects::plan_copy_project(
                &c.paths, &c.store, project, &library, &selected, &names,
            )?,
            skm_core::projects::ProjectMode::Link => skm_core::projects::plan_link_project(
                &c.paths, &c.store, project, &selected, &names,
            )?,
        });
    }
    plans.push(plan);
    let code = output::plan(c, json!({"remove":item,"off":off,"active":active}), &plans)?;
    if c.cli.dry_run || code != 0 {
        return Ok(code);
    }
    bind_global(c, plans.last_mut().unwrap())?;
    let code = output::apply(c, &plans)?;
    if code == 0 {
        for project in projects {
            skm_core::projects::remove_direct_item(&c.store, project.id, &item)?;
        }
    }
    skm_core::indexer::catch_up(&c.store, &c.paths, None)?;
    Ok(code)
}

pub fn doctor(
    cli: &crate::args::Cli,
    paths: &skm_core::paths::Paths,
    registry: &skm_core::registry::Registry,
    reindex: bool,
) -> Result<i32, Error> {
    let known: Vec<_> = registry.agents.keys().map(String::as_str).collect();
    match Library::load(&paths.library.join("library.toml"), &known) {
        Ok(_) => {}
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let counts = if reindex {
        let store = skm_core::store::Store::open(paths)?;
        let n = if cli.dry_run {
            skm_core::indexer::preview_reindex(&store, paths)?
        } else {
            skm_core::indexer::full_reindex(&store, paths)?
        };
        Some(
            json!({"total":n.total,"inserted":n.inserted,"updated":n.updated,"removed":n.removed,"unchanged":n.unchanged}),
        )
    } else {
        None
    };
    let entries = [
        ("home", &paths.home),
        ("library", &paths.library),
        ("cache", &paths.cache),
        ("trash", &paths.trash),
        ("logs", &paths.logs),
        ("config", &paths.config_file),
        ("db", &paths.db_file),
        ("lock", &paths.lock_file),
    ];
    let mut value = serde_json::Map::new();
    for (name, path) in entries {
        value.insert(name.into(), json!(path));
    }
    if let Some(n) = &counts {
        value.insert("reindex".into(), n.clone());
    }
    if cli.json {
        println!("{}", json!(value));
    } else if !cli.quiet {
        if let Some(n) = counts {
            println!(
                "reindex: {} total, {} inserted, {} updated, {} removed, {} unchanged",
                n["total"], n["inserted"], n["updated"], n["removed"], n["unchanged"]
            );
        }
        for (name, path) in entries {
            println!("{name}: {}", path.display());
        }
    }
    Ok(0)
}
