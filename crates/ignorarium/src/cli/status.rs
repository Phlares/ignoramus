//! `status`: how far a study has run and what it has spent. Text and JSON carry the same fields.
//! Reads only: it never creates the study's `run` folder.
use super::{CliError, Ctx, Done, Run, out, registered, stopped_why};
use crate::canon::sha256_hex;
use crate::control::{ControlFile, RunLock, Signal, State};
use crate::estimate::apply_defaults;
use crate::records::Record;
use crate::spec::model::Spec;
use crate::{machine, schedule, seed, spec};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The condition ids in a fixed order that depends on the spec hash and on nothing else:
/// each id sorts by a hash of its seed and itself.
pub fn blind_order(spec_hash: &[u8; 32], ids: &[String]) -> Vec<String> {
    let seed = seed::seed(spec_hash, "blind/conditions");
    let mut keyed: Vec<(String, &String)> = ids
        .iter()
        .map(|id| {
            let mut bytes = seed.to_vec();
            bytes.extend_from_slice(id.as_bytes());
            (sha256_hex(&bytes), id)
        })
        .collect();
    keyed.sort();
    keyed.into_iter().map(|(_, id)| id.clone()).collect()
}

/// `condition A`, `condition B`, and so on.
fn opaque_label(i: usize) -> String {
    let letter = u8::try_from(i % 26).map_or('?', |n| char::from(b'A' + n));
    let round = i / 26;
    if round == 0 {
        format!("condition {letter}")
    } else {
        format!("condition {letter}{round}")
    }
}

/// How the study stands: the first of a stop record, a held run lock, a dead run's
/// `running` state, then the last state a run wrote.
fn standing(
    ctx: &Ctx,
    id: &str,
    all: &[Record],
) -> Result<(&'static str, Option<String>), CliError> {
    let home = ctx.home()?;
    let io =
        |e: &dyn std::fmt::Display| CliError::usage(format!("cannot read the state of {id}: {e}"));
    if let Some(why) = stopped_why(all) {
        return Ok(("stopped", Some(why)));
    }
    if home.study_dir(id).join("run").is_dir() {
        match RunLock::try_take(home, id).map_err(|e| io(&e))? {
            None => return Ok(("running", None)),
            Some(lock) => drop(lock),
        }
    }
    Ok(match State::read(home, id).map_err(|e| io(&e))? {
        Some(State::Running) => ("interrupted", None),
        Some(State::Paused(why)) => ("paused", Some(why)),
        Some(State::Stopped(why)) => ("stopped", Some(why)),
        Some(State::Done) => ("done", None),
        None => ("not started", None),
    })
}

/// Every field of the report, as one JSON object.
pub fn report(ctx: &Ctx, id: &str) -> Result<Value, CliError> {
    let (home, _, all) = registered(ctx, id)?;
    let bad = |e: &dyn std::fmt::Display| CliError::usage(format!("cannot report {id}: {e}"));
    let raw = spec::load(&home.study_dir(id).join("spec.toml")).map_err(|e| bad(&e))?;
    let mut value = raw.value;
    apply_defaults(&mut value);
    let spec = Spec::from_value(&value).map_err(|e| bad(&e))?;
    let spec_hash = spec::hash_bytes(&raw.hash).ok_or_else(|| bad(&"bad spec hash"))?;
    let machine = machine::load(&home.root).map_err(|e| bad(&e))?;
    let rows = schedule::expand(&spec, &spec_hash, &schedule::units_from_spec(&spec))
        .map_err(|e| bad(&e))?;

    let recorded: Vec<&Record> = all.iter().filter(|r| r.kind == "row").collect();
    let num = |r: &Record, k: &str| r.body[k].as_f64().unwrap_or(0.0);
    let spent: Vec<&Record> = all
        .iter()
        .filter(|r| r.kind == "row" || r.kind == "spend")
        .collect();
    let cpu_seconds: f64 = spent.iter().map(|r| num(r, "cpu_seconds")).sum();
    let wall_seconds: f64 = spent.iter().map(|r| num(r, "wall_seconds")).sum();
    let done_rows: BTreeSet<u64> = recorded.iter().filter_map(|r| r.row).collect();
    let (done, total) = (done_rows.len(), rows.len());
    let voided = recorded.iter().filter(|r| r.body["exit"] != "ok").count();
    let retried: u64 = recorded
        .iter()
        .map(|r| r.body["attempts"].as_u64().unwrap_or(1).saturating_sub(1))
        .sum();

    let hours = wall_seconds / 3600.0;
    let money = hours * machine.rate_usd_per_hour;
    let per_hour = (done > 0 && hours > 0.0).then(|| done as f64 / hours);
    let eta = if done >= total {
        Some(0.0)
    } else {
        per_hour.map(|r| (total - done) as f64 / r * 3600.0)
    };
    let budget = &spec.scope.budget;
    let cost = json!({
        "cpu_hours": cpu_seconds / 3600.0,
        "wall_hours": hours,
        "money_usd": money,
    });
    let fraction = |spent: f64, cap: f64| (cap > 0.0).then(|| spent / cap);

    let ids: Vec<String> = spec
        .design
        .conditions
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let blind = spec.access.blind;
    let shown: Vec<(String, String)> = if blind {
        blind_order(&spec_hash, &ids)
            .into_iter()
            .enumerate()
            .map(|(i, real)| (real, opaque_label(i)))
            .collect()
    } else {
        ids.iter().map(|i| (i.clone(), i.clone())).collect()
    };
    let mut counts: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for r in &rows {
        counts.entry(r.condition.as_str()).or_default().1 += 1;
    }
    let mut seen = BTreeSet::new();
    for r in &recorded {
        if let (Some(i), Some(c)) = (r.row, r.body["condition"].as_str())
            && seen.insert(i)
        {
            counts.entry(c).or_default().0 += 1;
        }
    }
    let conditions: Vec<Value> = shown
        .iter()
        .map(|(real, label)| {
            let (d, t) = counts.get(real.as_str()).copied().unwrap_or_default();
            json!({ "label": label, "done": d, "total": t })
        })
        .collect();

    let (state, detail) = standing(ctx, id, &all)?;
    let request = match ControlFile::of(home, id).peek().map_err(|e| bad(&e))? {
        Some(Signal::Pause) => Some("pause"),
        Some(Signal::Resume) => Some("resume"),
        Some(Signal::Stop(_)) => Some("stop"),
        None => None,
    };
    Ok(json!({
        "study": id,
        "state": state,
        "detail": detail,
        "request": request,
        "done": done,
        "total": total,
        "rows_per_hour": per_hour,
        "elapsed_seconds": wall_seconds,
        "eta_seconds": eta,
        "cost": cost,
        "budget": {
            "cpu_hours": budget.cpu_hours,
            "wall_hours": budget.wall_hours,
            "money_usd": budget.money_usd,
        },
        "used": {
            "cpu_hours": fraction(cpu_seconds / 3600.0, budget.cpu_hours),
            "wall_hours": fraction(hours, budget.wall_hours),
            "money_usd": fraction(money, budget.money_usd),
        },
        "cost_rate_usd_per_hour": (hours > 0.0).then(|| money / hours),
        "voided": voided,
        "retried": retried,
        "triggers": [],
        "blind": blind,
        "conditions": conditions,
    }))
}

fn num_or_na(v: &Value, unit: &str) -> String {
    v.as_f64()
        .map_or("n/a".into(), |n| format!("{n:.2} {unit}"))
}

fn percent(v: &Value) -> String {
    v.as_f64()
        .map_or("no cap".into(), |f| format!("{:.0}%", f * 100.0))
}

fn render(r: &Value) -> String {
    let mut s = format!(
        "study: {}\nstate: {}\n",
        r["study"].as_str().unwrap_or(""),
        r["state"].as_str().unwrap_or("")
    );
    if let Some(d) = r["detail"].as_str() {
        s.push_str(&format!("reason: {d}\n"));
    }
    if let Some(q) = r["request"].as_str() {
        s.push_str(&format!("request: {q}\n"));
    }
    s.push_str(&format!("rows: {}/{}\n", r["done"], r["total"]));
    s.push_str(&format!(
        "rate: {}\n",
        num_or_na(&r["rows_per_hour"], "rows/hour")
    ));
    s.push_str(&format!(
        "elapsed: {}\n",
        num_or_na(&r["elapsed_seconds"], "s")
    ));
    s.push_str(&format!("eta: {}\n", num_or_na(&r["eta_seconds"], "s")));
    let c = &r["cost"];
    s.push_str(&format!(
        "cost: cpu {} h, wall {} h, money ${}\n",
        num(&c["cpu_hours"]),
        num(&c["wall_hours"]),
        num(&c["money_usd"])
    ));
    let (b, u) = (&r["budget"], &r["used"]);
    s.push_str(&format!(
        "budget: cpu {} h ({}), wall {} h ({}), money ${} ({})\n",
        num(&b["cpu_hours"]),
        percent(&u["cpu_hours"]),
        num(&b["wall_hours"]),
        percent(&u["wall_hours"]),
        num(&b["money_usd"]),
        percent(&u["money_usd"]),
    ));
    s.push_str(&format!(
        "cost rate: {}\n",
        num_or_na(&r["cost_rate_usd_per_hour"], "USD/hour")
    ));
    s.push_str(&format!(
        "voided: {}\nretried: {}\n",
        r["voided"], r["retried"]
    ));
    let triggers = r["triggers"].as_array().map_or(&[][..], Vec::as_slice);
    if triggers.is_empty() {
        s.push_str("triggers: none\n");
    }
    for c in r["conditions"].as_array().map_or(&[][..], Vec::as_slice) {
        s.push_str(&format!(
            "{}: {}/{}\n",
            c["label"].as_str().unwrap_or(""),
            c["done"],
            c["total"]
        ));
    }
    s
}

fn num(v: &Value) -> String {
    format!("{:.3}", v.as_f64().unwrap_or(0.0))
}

pub fn run(ctx: &Ctx, id: &str, json: bool) -> Run {
    let r = report(ctx, id)?;
    let text = if json {
        serde_json::to_string_pretty(&r).map_err(|e| CliError::usage(e.to_string()))? + "\n"
    } else {
        render(&r)
    };
    out(&text)?;
    Ok(Done::exit(0))
}
