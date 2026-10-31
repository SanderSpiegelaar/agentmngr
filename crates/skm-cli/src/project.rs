use crate::{Context, args::ProjectCommand, commands, output};
use serde_json::json;
use skm_core::{
    Error,
    engine::Plan,
    library::{ItemSelection, Library},
    projects::{self, Project, ProjectMode},
    target::{ItemKind, ItemRef},
};
use std::path::Path;

pub fn find(c: &Context, path: &Path) -> Result<Project, Error> {
    let path = path.canonicalize()?;
    projects::list(&c.store)?
        .into_iter()
        .find(|p| p.path == path)
        .ok_or_else(|| Error::ConfigParse(format!("project {} is not registered", path.display())))
}

pub fn selection(
    c: &Context,
    library: &Library,
    project: &Project,
    sets: &[String],
    direct: &ItemSelection,
) -> Result<ItemSelection, Error> {
    let enabled = c.store.read(|db| {
        let mut q = db.prepare("SELECT id FROM agents WHERE enabled=1")?;
        Ok(q.query_map([], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?)
    })?;
    let _ = project;
    projects::effective(library, sets, direct, &c.registry, &enabled)
}

pub fn direct(c: &Context, p: &Project) -> Result<ItemSelection, Error> {
    let mut direct = ItemSelection::default();
    for entry in projects::direct_items(&c.store, p.id)? {
        direct.set_item(&entry.item, entry.agents.as_deref())?;
    }
    Ok(direct)
}

pub fn plan(
    c: &Context,
    project: &Project,
    library: &Library,
    selection: &ItemSelection,
) -> Result<Plan, Error> {
    projects::plan_staged_project(
        &c.paths,
        &c.store,
        project,
        library,
        selection,
        &Default::default(),
    )
}

pub fn toggle(
    c: &Context,
    item: &ItemRef,
    path: &Path,
    agents: &[String],
    on: bool,
) -> Result<i32, Error> {
    if item.kind == ItemKind::Plugin {
        return Err(Error::ConfigParse("plugins are global-only".into()));
    }
    let project = find(c, path)?;
    let library = c.library()?;
    let mut direct = direct(c, &project)?;
    if on {
        direct.set_item(
            item,
            if agents.is_empty() {
                None
            } else {
                Some(agents)
            },
        )?;
    } else {
        direct.remove_item(item);
    }
    let sets = projects::used_sets(&c.store, project.id)?;
    let selection = selection(c, &library, &project, &sets, &direct)?;
    if !on && item.kind == ItemKind::Skill && selection.skills.contains(&item.name) {
        return Err(Error::ConfigParse(
            "skill is selected by a set; remove it from the set or unuse the set".into(),
        ));
    }
    let names = if item.kind == ItemKind::Skill {
        std::collections::BTreeSet::from([item.name.clone()])
    } else {
        Default::default()
    };
    let plan = match project.mode {
        ProjectMode::Copy => {
            projects::plan_copy_project(&c.paths, &c.store, &project, &library, &selection, &names)?
        }
        ProjectMode::Link => {
            projects::plan_link_project(&c.paths, &c.store, &project, &selection, &names)?
        }
    };
    let code = output::plan(
        c,
        json!({"project":project.path,"item":item,"on":on,"agents":agents}),
        std::slice::from_ref(&plan),
    )?;
    if c.cli.dry_run {
        return Ok(code);
    }
    if on {
        projects::set_direct_item(
            &c.store,
            &c.registry,
            project.id,
            item,
            if agents.is_empty() {
                None
            } else {
                Some(agents)
            },
        )?;
    } else {
        projects::remove_direct_item(&c.store, project.id, item)?;
    }
    output::apply(c, &[plan])
}

pub fn run(c: &Context, command: &ProjectCommand) -> Result<i32, Error> {
    if matches!(command, ProjectCommand::List) {
        let projects = projects::list(&c.store)?;
        return output::data(c, &projects, || {
            println!("PATH\tMODE\tAUTO-SYNC");
            for p in &projects {
                println!("{}\t{:?}\t{}", p.path.display(), p.mode, p.auto_sync);
            }
        });
    }
    if let ProjectCommand::Add { path } = command {
        let (registration, plan) = projects::plan_registration(&c.store, path)?;
        let code = output::plan(
            c,
            json!({"register":registration.project.path,"suggested_sets":registration.reattachment.suggested_sets}),
            std::slice::from_ref(&plan),
        )?;
        if !c.cli.dry_run {
            projects::register_with_report(&c.store, path)?;
            if plan.actions.is_empty() {
                commands::changed(c)?;
            }
        }
        return Ok(code);
    }
    let path = match command {
        ProjectCommand::Remove { path, .. }
        | ProjectCommand::Use { path, .. }
        | ProjectCommand::Unuse { path, .. }
        | ProjectCommand::Mode { path, .. }
        | ProjectCommand::Pause { path }
        | ProjectCommand::Resume { path } => path,
        _ => unreachable!(),
    };
    let mut project = find(c, path)?;
    let library = c.library()?;
    let mut sets = projects::used_sets(&c.store, project.id)?;
    let mut direct = direct(c, &project)?;
    let changes;
    match command {
        ProjectCommand::Remove { clean, .. } => {
            changes = json!({"remove_project":project.path,"clean":clean});
            if *clean {
                sets.clear();
                direct = ItemSelection::default();
            }
        }
        ProjectCommand::Use { sets: names, .. } => {
            for name in names {
                if !library.sets.contains_key(name) {
                    return Err(Error::ConfigParse(format!("unknown set {name:?}")));
                }
                if !sets.contains(name) {
                    sets.push(name.clone());
                }
            }
            changes = json!({"project":project.path,"use":names});
        }
        ProjectCommand::Unuse { sets: names, .. } => {
            sets.retain(|name| !names.contains(name));
            changes = json!({"project":project.path,"unuse":names});
        }
        ProjectCommand::Mode { mode, .. } => {
            project.mode = if mode == "link" {
                ProjectMode::Link
            } else {
                ProjectMode::Copy
            };
            changes = json!({"project":project.path,"mode":project.mode});
        }
        ProjectCommand::Pause { .. } => {
            project.auto_sync = false;
            changes = json!({"project":project.path,"auto_sync":false});
        }
        ProjectCommand::Resume { .. } => {
            project.auto_sync = true;
            changes = json!({"project":project.path,"auto_sync":true});
        }
        _ => unreachable!(),
    }
    let plans = if matches!(
        command,
        ProjectCommand::Pause { .. } | ProjectCommand::Remove { clean: false, .. }
    ) {
        vec![]
    } else {
        let selected = selection(c, &library, &project, &sets, &direct)?;
        vec![plan(c, &project, &library, &selected)?]
    };
    let code = output::plan(c, changes, &plans)?;
    if c.cli.dry_run || code != 0 {
        return Ok(code);
    }
    if matches!(command, ProjectCommand::Resume { .. }) {
        return match projects::sync::resume(&c.paths, &c.store, &library, &c.registry, project.id)?
        {
            projects::sync::SyncOutcome::Applied { result, .. } => Ok(output::applied(c, &result)),
            projects::sync::SyncOutcome::Failed { error, .. } => Err(error),
            projects::sync::SyncOutcome::Refused { issues, .. } => output::plan(
                c,
                serde_json::Value::Null,
                &[Plan {
                    issues,
                    ..Plan::default()
                }],
            ),
            projects::sync::SyncOutcome::Pending { .. } => {
                Err(Error::ConfigChanged("project remained paused".into()))
            }
        };
    }
    let code = output::apply(c, &plans)?;
    if code != 0 {
        return Ok(code);
    }
    match command {
        ProjectCommand::Remove { .. } => projects::unregister(&c.store, project.id)?,
        ProjectCommand::Use { sets, .. } => {
            for name in sets {
                projects::use_set(&c.store, &library, project.id, name)?;
            }
        }
        ProjectCommand::Unuse { sets, .. } => {
            for name in sets {
                projects::unuse_set(&c.store, project.id, name)?;
            }
        }
        ProjectCommand::Mode { .. } => projects::set_mode(&c.store, project.id, project.mode)?,
        ProjectCommand::Pause { .. } => projects::sync::pause(&c.store, project.id)?,
        _ => unreachable!(),
    }
    if plans.is_empty() {
        commands::changed(c)?;
    }
    Ok(0)
}
