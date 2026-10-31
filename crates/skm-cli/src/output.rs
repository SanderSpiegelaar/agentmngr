use crate::Context;
use serde::Serialize;
use serde_json::{Value, json};
use skm_core::{
    Error,
    engine::{ApplyResult, Plan},
};

pub fn data(context: &Context, value: &impl Serialize, human: impl FnOnce()) -> Result<i32, Error> {
    if context.cli.json {
        println!(
            "{}",
            serde_json::to_string(value).map_err(|e| Error::ConfigParse(e.to_string()))?
        );
    } else if !context.cli.quiet {
        human();
    }
    Ok(0)
}

pub fn plan(context: &Context, changes: Value, plans: &[Plan]) -> Result<i32, Error> {
    let issues: Vec<_> = plans.iter().flat_map(|plan| &plan.issues).collect();
    if context.cli.json {
        let plans: Vec<_> = plans
            .iter()
            .map(|plan| {
                json!({
                    "summary": plan.summary,
                    "actions": plan.actions.iter().map(|action| &action.action).collect::<Vec<_>>(),
                    "issues": plan.issues,
                })
            })
            .collect();
        let mut value = json!({"ok":issues.is_empty(),"changes":changes,"plans":plans});
        if let Some(issue) = issues.first() {
            value["code"] = json!(issue.code);
            value["message"] = json!(issue.message);
            value["fix"] = json!("Resolve the reported issues and retry.");
        }
        *context.report.borrow_mut() = Some(value);
    } else if !context.cli.quiet {
        if let Some(changes) = changes.as_object() {
            for (key, value) in changes {
                let text = match value {
                    Value::Null => continue,
                    Value::String(text) => text.clone(),
                    Value::Array(values) => values
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| v.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    Value::Object(item)
                        if item.contains_key("kind") && item.contains_key("name") =>
                    {
                        format!(
                            "{} {}",
                            item["kind"].as_str().unwrap_or_default(),
                            item["name"].as_str().unwrap_or_default()
                        )
                    }
                    value => value.to_string(),
                };
                println!("{key}: {text}");
            }
        }
        for plan in plans {
            println!("{}", plan.summary);
            for action in &plan.actions {
                match &action.action {
                    skm_core::engine::Action::WriteFile { target, bytes } => {
                        println!("WriteFile {} ({} bytes)", target.display(), bytes.len())
                    }
                    action => println!("{action:?}"),
                }
            }
        }
    }
    for issue in &issues {
        if !context.cli.json {
            eprintln!(
                "issue: {}: {}: {}",
                issue.code,
                issue.target.display(),
                issue.message
            );
        }
    }
    Ok(if issues.is_empty() { 0 } else { 3 })
}

pub fn applied(context: &Context, result: &ApplyResult) -> i32 {
    if context.cli.json
        && let Some(issue) = result.failed.first().or_else(|| result.issues.first())
        && let Some(value) = context.report.borrow_mut().as_mut()
    {
        value["ok"] = json!(false);
        value["code"] = json!(issue.code);
        value["message"] = json!(issue.message);
        value["fix"] = json!("Resolve the reported issues and retry.");
        value["done"] = json!(result.done);
        value["failed"] = json!(result.failed);
    }
    if !context.cli.json && !context.cli.quiet && !result.failed.is_empty() {
        println!(
            "{} actions, {} issues, {} failed",
            result.done.len(),
            result.issues.len(),
            result.failed.len()
        );
    }
    for issue in result.issues.iter().chain(&result.failed) {
        if !context.cli.json {
            eprintln!(
                "{}: {}: {}",
                issue.code,
                issue.target.display(),
                issue.message
            );
        }
    }
    if !result.failed.is_empty() {
        1
    } else if !result.issues.is_empty() {
        3
    } else {
        0
    }
}

pub fn apply(context: &Context, plans: &[Plan]) -> Result<i32, Error> {
    for plan in plans {
        let result = skm_core::engine::apply(&context.paths, &context.store, plan, "cli", None)?;
        let next = applied(context, &result);
        if next != 0 {
            return Ok(next);
        }
    }
    Ok(0)
}
