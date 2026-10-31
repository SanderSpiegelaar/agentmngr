use crate::{Context, args::SetCommand, commands, output};
use serde_json::json;
use skm_core::{Error, projects};

pub fn run(c: &Context, command: &SetCommand) -> Result<i32, Error> {
    let before = c.library()?;
    if matches!(command, SetCommand::List) {
        return output::data(c, &before.sets, || {
            for name in before.sets.keys() {
                println!("{name}");
            }
        });
    }
    let name = match command {
        SetCommand::Show { name }
        | SetCommand::Create { name }
        | SetCommand::Delete { name }
        | SetCommand::Add { name, .. }
        | SetCommand::Rm { name, .. } => name,
        _ => unreachable!(),
    };
    if matches!(command, SetCommand::Show { .. }) {
        let set = before
            .sets
            .get(name)
            .ok_or_else(|| Error::ConfigParse(format!("unknown set {name:?}")))?;
        return output::data(c, set, || println!("{}", json!(set)));
    }
    let mut after = before.clone();
    match command {
        SetCommand::Create { .. } => after.create_set(name, None)?,
        SetCommand::Delete { .. } => {
            after.delete_set(name)?;
        }
        SetCommand::Add { items, .. } | SetCommand::Rm { items, .. } => {
            let agent = match command {
                SetCommand::Add { agent, .. } => agent.as_slice(),
                _ => &[],
            };
            let mut set = after
                .sets
                .get(name)
                .cloned()
                .ok_or_else(|| Error::ConfigParse(format!("unknown set {name:?}")))?;
            for text in items {
                let item = commands::item_ref(c, text)?;
                if matches!(command, SetCommand::Add { .. }) {
                    set.items
                        .set_item(&item, if agent.is_empty() { None } else { Some(agent) })?;
                } else {
                    set.items.remove_item(&item);
                }
            }
            after.update_set(name, set, &c.known_agents())?;
        }
        _ => unreachable!(),
    }
    let affected =
        projects::plan_set_change(&c.paths, &c.store, &before, &after, name, &c.registry)?;
    let mut plans = vec![commands::manifest_plan(c, &after)?];
    plans.extend(
        affected
            .into_iter()
            .filter(|p| p.project.auto_sync)
            .map(|p| p.plan),
    );
    let code = output::plan(
        c,
        json!({"set":name,"definition":after.sets.get(name)}),
        &plans,
    )?;
    if c.cli.dry_run || code != 0 {
        return Ok(code);
    }
    commands::bind_global(c, &mut plans[0])?;
    let code = output::apply(c, &plans)?;
    commands::pending_set(c, name)?;
    Ok(code)
}
