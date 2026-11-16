//! `status`: how far a study has run and what it has spent. Text and JSON carry the same fields.
//! Reads only: it never creates the study's `run` folder.
use super::{CliError, Ctx, Done, Run, out, registered, stopped_why};
use crate::canon::sha256_hex;
use crate::control::{ControlFile, Signal, State, run_is_live};
use crate::derived::{self, DoneRecord, Register, SpawnStop, criterion_state};
use crate::diag::printable;
use crate::home::Home;
use crate::records::Record;
use crate::runner::progress;
use crate::runner::{Progress, RunError};
use crate::spec::model::{SELECTED, Spec, Trigger};
use crate::telemetry::{RowView, overview};
use crate::{resize, seed, spawn};
use serde_json::{Value, json};

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
    if run_is_live(home, id).map_err(|e| io(&e))? {
        return Ok(("running", None));
    }
    Ok(match State::read(home, id).map_err(|e| io(&e))? {
        Some(State::Running) => ("interrupted", None),
        Some(State::Paused(why)) => ("paused", Some(why)),
        Some(State::Stopped(why)) => ("stopped", Some(why)),
        Some(State::Done) => ("done", None),
        None => ("not started", None),
    })
}

/// Each condition of `spec` with the label status shows for it: its id, or when the study is
/// blind, an opaque label in an order that depends on the spec hash alone.
pub fn shown_labels(spec: &Spec, spec_hash: &[u8; 32]) -> Vec<(String, String)> {
    let ids: Vec<String> = (spec.design.conditions.iter())
        .map(|c| c.id.clone())
        .collect();
    if spec.access.blind {
        blind_order(spec_hash, &ids)
            .into_iter()
            .enumerate()
            .map(|(i, real)| (real, opaque_label(i)))
            .collect()
    } else {
        ids.iter().map(|i| (i.clone(), i.clone())).collect()
    }
}

/// `id` as `shown` labels it: a condition id by its label, a child id (its parent's id, `~`,
/// then its operator and value) with the parent's label in front. The `~` prefix finds the
/// parent because a condition id never holds `~` (its format rule).
pub fn relabelled(shown: &[(String, String)], id: &str) -> String {
    let split = |(real, label): &(String, String)| {
        if id == real {
            return Some(label.clone());
        }
        let rest = id.strip_prefix(&format!("{real}~"))?;
        Some(format!("{label}~{rest}"))
    };
    shown.iter().find_map(split).unwrap_or_else(|| id.into())
}

/// Each trigger of `spec` in spec order: its decision when a `trigger` record holds one, else
/// `pending`; a trigger of the old shape is `inert (old shape)`. Children show by the label of
/// their condition.
fn triggers(
    spec: &Spec,
    all: &[Record],
    shown: &[(String, String)],
) -> Result<Vec<Value>, RunError> {
    let recorded = spawn::recorded(all)?;
    let label = |ids: &[String]| ids.iter().map(|i| relabelled(shown, i)).collect::<Vec<_>>();
    let skipped = |t: &derived::Trigger| {
        let each = |k: &derived::Skipped| {
            let child = relabelled(shown, &k.child);
            json!({ "child": child, "reason": k.reason })
        };
        t.skipped.iter().map(each).collect::<Vec<_>>()
    };
    let entry = |id: &str, state: &str, threshold: f64, t: Option<&derived::Trigger>| {
        json!({
            "id": id, "state": state, "e_value": t.map(|t| t.e_value), "threshold": threshold,
            "reason": t.and_then(|t| t.reason.clone()),
            "admitted": t.map(|t| label(&t.admitted)).unwrap_or_default(),
            "skipped": t.map(skipped).unwrap_or_default(),
        })
    };
    let each = |t: &Trigger| match t {
        Trigger::Legacy(l) => entry(&l.id, "inert (old shape)", l.threshold_e, None),
        Trigger::Declared(d) => {
            let done = recorded.iter().find(|r| r.id == d.id);
            let state = match done {
                None => "pending",
                Some(r) if r.fired => "fired",
                Some(_) => "not fired",
            };
            entry(&d.id, state, d.threshold_e, done)
        }
    };
    Ok(spec.mutations.triggers.iter().map(each).collect())
}

fn cannot_report(id: &str, e: &dyn std::fmt::Display) -> CliError {
    CliError::usage(format!("cannot report {id}: {e}"))
}

/// A registered study's home, verified records and progress.
fn opened<'a>(ctx: &'a Ctx, id: &str) -> Result<(&'a Home, Vec<Record>, Progress), CliError> {
    let (home, _, all) = registered(ctx, id)?;
    let p = progress(home, id, &all).map_err(|e| cannot_report(id, &e))?;
    Ok((home, all, p))
}

/// Every field of the report, as one JSON object.
pub fn report(ctx: &Ctx, id: &str) -> Result<Value, CliError> {
    let (home, all, p) = opened(ctx, id)?;
    let bad = |e: &dyn std::fmt::Display| cannot_report(id, e);
    let (spec, spec_hash, done, total) = (&p.spec, p.spec_hash, p.done, p.total);
    let wall_seconds = p.cost.wall_seconds;
    let hours = wall_seconds / 3600.0;
    let (cpu_hours, money) = (p.cost.cpu_seconds / 3600.0, p.cost.usd);
    let per_hour = (done > 0 && hours > 0.0).then(|| done as f64 / hours);
    let eta = if done >= total {
        Some(0.0)
    } else {
        per_hour.map(|r| (total - done) as f64 / r * 3600.0)
    };
    let budget = &spec.scope.budget;
    let cost = json!({ "cpu_hours": cpu_hours, "wall_hours": hours, "money_usd": money });
    let fraction = |spent: f64, cap: f64| (cap > 0.0).then(|| spent / cap);

    let blind = spec.access.blind;
    let shown = shown_labels(spec, &spec_hash);
    // The spec's conditions first, then every other counted condition, so the lines sum to
    // `done` and `total`. The extra lines sort by shown label with `selected` last: real-id
    // order would show a blind reader which real ids sort before which. `relabelled` leaves the
    // literal `selected` as is.
    let spec_lines = shown
        .iter()
        .map(|(real, label)| (real.as_str(), label.clone()));
    let mut extra_lines: Vec<(&str, String)> = (p.conditions.keys())
        .filter(|k| !shown.iter().any(|(real, _)| real == *k))
        .map(|k| (k.as_str(), relabelled(&shown, k)))
        .collect();
    extra_lines.sort_by(|a, b| ((a.0 == SELECTED), &a.1).cmp(&(b.0 == SELECTED, &b.1)));
    let conditions: Vec<Value> = spec_lines
        .chain(extra_lines)
        .map(|(real, label)| {
            let (d, t) = p.conditions.get(real).copied().unwrap_or_default();
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
    // The re-size shows its decision and counts alone: no spread, mean or e-value.
    let resize = resize::recorded(&all).map_err(|e| bad(&e))?.map(|r| {
        let strata: Vec<Value> = (r.strata.iter())
            .map(|g| {
                json!({
                    "stratum": g.stratum, "first": g.first, "needed": g.needed,
                    "appended": g.appended,
                })
            })
            .collect();
        json!({ "decision": r.decision, "strata": strata })
    });
    let triggers = triggers(spec, &all, &shown).map_err(|e| bad(&e))?;
    let ended = (all.iter()).find_map(|r| {
        let d = DoneRecord::of(r)?;
        Some(json!({ "end": d.end, "ended_at": d.ended_at, "seq": r.seq }))
    });
    let replicates = (all.iter().find_map(Register::of))
        .and_then(|g| g.replicates)
        .map(|r| {
            json!({
                "study": r.study, "done": r.done, "hypothesis": r.hypothesis,
                "criterion": r.criterion, "result": criterion_state(&all, &r),
            })
        });
    let stopped_children: Vec<String> = (all.iter().filter_map(SpawnStop::of))
        .flat_map(|s| s.children)
        .map(|c| relabelled(&shown, &c))
        .collect();
    Ok(json!({
        "study": id,
        "state": state,
        "detail": detail,
        "request": request,
        "ended": ended,
        "replicates": replicates,
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
            "cpu_hours": fraction(cpu_hours, budget.cpu_hours),
            "wall_hours": fraction(hours, budget.wall_hours),
            "money_usd": fraction(money, budget.money_usd),
        },
        "cost_rate_usd_per_hour": (hours > 0.0).then(|| money / hours),
        "voided": p.voided,
        "retried": p.retried,
        "unknown_cpu": p.unknown_cpu,
        "triggers": triggers,
        "spawn_stopped": stopped_children,
        "blind": blind,
        "conditions": conditions,
        "resize": resize,
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
    if let (Some(end), Some(at)) = (r["ended"]["end"].as_str(), r["ended"]["ended_at"].as_str()) {
        s.push_str(&format!("ended: {end} at {at}\n"));
    }
    let link = &r["replicates"];
    if let Some(result) = link["result"].as_str() {
        let at = |k: &str| link[k].as_str().unwrap_or_default();
        s.push_str(&format!(
            "replicates {} ({}): criterion {}: {result}\n",
            at("study"),
            at("hypothesis"),
            at("criterion")
        ));
    }
    s.push_str(&format!("rows: {}/{}\n", r["done"], r["total"]));
    if let Some(d) = r["resize"]["decision"].as_str() {
        s.push_str(&format!("resize: {d}\n"));
    }
    for g in r["resize"]["strata"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
    {
        let name = g["stratum"].as_str().filter(|n| !n.is_empty());
        let needed = g["needed"].as_u64().map_or("n/a".into(), |n| n.to_string());
        s.push_str(&format!(
            "resize {}: first {}, needed {needed}, appended {}\n",
            name.unwrap_or("pooled"),
            g["first"],
            g["appended"]
        ));
    }
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
    let blind = if r["blind"] == true { "yes" } else { "no" };
    s.push_str(&format!("blind: {blind}\n"));
    s.push_str(&format!("rows with unknown cpu: {}\n", r["unknown_cpu"]));
    let triggers = r["triggers"].as_array().map_or(&[][..], Vec::as_slice);
    if triggers.is_empty() {
        s.push_str("triggers: none\n");
    }
    for t in triggers {
        s.push_str(&trigger_line(t));
    }
    let stopped = joined(names(&r["spawn_stopped"]));
    if stopped != "none" {
        s.push_str(&format!("children stopped by the budget: {stopped}\n"));
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

/// `list` joined, or `none` when it is empty.
fn joined(list: impl Iterator<Item = String>) -> String {
    let list: Vec<String> = list.collect();
    if list.is_empty() {
        "none".into()
    } else {
        list.join(", ")
    }
}

/// The strings of the JSON list `v`, made printable.
fn names(v: &Value) -> impl Iterator<Item = String> + '_ {
    (v.as_array().into_iter().flatten()).filter_map(|x| x.as_str().map(printable))
}

/// `T1: fired, e-value 3.000, threshold 2.000, admitted a, skipped b (reason)`. A trigger
/// with no decision shows its state alone.
fn trigger_line(t: &Value) -> String {
    let id = printable(t["id"].as_str().unwrap_or(""));
    let state = t["state"].as_str().unwrap_or("");
    let Some(e) = t["e_value"].as_f64() else {
        return format!("{id}: {state}\n");
    };
    let why = t["reason"]
        .as_str()
        .map_or(String::new(), |w| format!(" ({w})"));
    let skipped = (t["skipped"].as_array().into_iter().flatten()).map(|k| {
        let (child, reason) = (&k["child"], &k["reason"]);
        format!(
            "{} ({})",
            printable(child.as_str().unwrap_or("")),
            printable(reason.as_str().unwrap_or(""))
        )
    });
    format!(
        "{id}: {state}{why}, e-value {e:.3}, threshold {}, admitted {}, skipped {}\n",
        num(&t["threshold"]),
        joined(names(&t["admitted"])),
        joined(skipped),
    )
}

fn num(v: &Value) -> String {
    format!("{:.3}", v.as_f64().unwrap_or(0.0))
}

/// A report builder: `report` or `telemetry_report`.
type Build = fn(&Ctx, &str) -> Result<Value, CliError>;

pub fn run(ctx: &Ctx, id: &str, json: bool, telemetry: bool) -> Run {
    let (build, render): (Build, fn(&Value) -> String) = if telemetry {
        (telemetry_report, render_telemetry)
    } else {
        (report, render)
    };
    let r = build(ctx, id)?;
    let text = if json {
        serde_json::to_string_pretty(&r).map_err(|e| CliError::usage(e.to_string()))? + "\n"
    } else {
        render(&r)
    };
    out(&text)?;
    Ok(Done::exit(0))
}

/// The host readings of the study's row records, from the typed row view alone: no value, hash
/// or output of a row reaches this report.
pub fn telemetry_report(ctx: &Ctx, id: &str) -> Result<Value, CliError> {
    let (_, all, p) = opened(ctx, id)?;
    let views: Vec<RowView> = all.iter().filter_map(RowView::of).collect();
    let blind = p.spec.access.blind;
    Ok(json!({
        "study": id,
        "blind": blind,
        "telemetry": overview(&views, &p.indices, blind),
    }))
}

/// `max 2.00 s, mean 1.00 s`: the largest and mean of the `{max, mean}` object `v`, with
/// `unit` after each and `precision` decimals; `n/a` when no row gave a figure.
fn spread_text(v: &Value, unit: &str, precision: usize) -> String {
    match (v["max"].as_f64(), v["mean"].as_f64()) {
        (Some(a), Some(b)) => format!("max {a:.precision$}{unit}, mean {b:.precision$}{unit}"),
        _ => "n/a".into(),
    }
}

fn render_telemetry(r: &Value) -> String {
    let t = &r["telemetry"];
    let seconds = |v: &Value| spread_text(v, " s", 2);
    let mut s = format!(
        "study: {}\nblind: {}\nrows: {}\n",
        printable(r["study"].as_str().unwrap_or("")),
        if r["blind"] == true { "yes" } else { "no" },
        t["rows"]
    );
    let states = |v: &Value| {
        joined((v.as_object().into_iter().flatten()).map(|(k, n)| format!("{} {n}", printable(k))))
    };
    let machine = t.get("cgroup_machine");
    let below = t["cgroup"].as_object().is_some_and(|m| !m.is_empty());
    if machine.is_none() || below {
        s.push_str(&format!("cgroup: {}\n", states(&t["cgroup"])));
    }
    if let Some(m) = machine {
        s.push_str(&format!("cgroup, level 2: {}\n", states(m)));
    }
    for h in t["host_cpu"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "host cpu: {} (flags {}), {} rows\n",
            printable(h["model"].as_str().unwrap_or("")),
            printable(h["flags_sha256"].as_str().unwrap_or("")),
            h["rows"]
        ));
    }
    let missing = |m: &Value| {
        format!(
            "{} unavailable, {} not recorded",
            m["unavailable"], m["not_recorded"]
        )
    };
    s.push_str(&format!(
        "rows with no host cpu: {}\n",
        missing(&t["host_cpu_missing"])
    ));
    for (name, key) in [("steal", "steal_s"), ("iowait", "iowait_s")] {
        s.push_str(&format!(
            "{name} per row: {} ({} rows)\n",
            seconds(&t[key]),
            t[key]["rows"]
        ));
    }
    s.push_str(&format!(
        "rows with no usable ticks: {}\n",
        missing(&t["ticks_missing"])
    ));
    for (name, key) in [("wall", "wall_s"), ("cpu", "cpu_s")] {
        s.push_str(&format!("{name} per row: {}\n", seconds(&t[key])));
    }
    for (i, b) in t["blocks"].as_array().into_iter().flatten().enumerate() {
        s.push_str(&format!(
            "block {} (schedule rows from {} to {}): {} rows, steal share {}, iowait share {}",
            i + 1,
            b["first_row"],
            b["last_row"],
            b["rows"],
            spread_text(&b["steal_share"], "", 3),
            spread_text(&b["iowait_share"], "", 3),
        ));
        if b.get("wall_s").is_some() {
            s.push_str(&format!(
                ", wall {}, cpu {}",
                seconds(&b["wall_s"]),
                seconds(&b["cpu_s"])
            ));
        }
        s.push('\n');
    }
    s
}
