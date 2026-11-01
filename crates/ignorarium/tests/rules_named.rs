mod common;

use ignorarium::canon::sha256_hex;
use ignorarium::derived::{Carried, Instrument, Recheck};
use ignorarium::diag::{Diagnostic, Severity};
use ignorarium::prior::{NoPriorUse, PriorError, PriorUse, normalized_sha256};
use ignorarium::records;
use ignorarium::rules::{
    self, Borrowed, BoxRecords, CheckCtx, Lookup, Power, Rule, StratumInput, estimate_inputs,
};
use ignorarium::stream::Next;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

fn v0() -> &'static [Rule] {
    static RULES: std::sync::OnceLock<Vec<Rule>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| rules::load("../../rules/v0.toml".as_ref()).unwrap())
}

fn spec_at(path: &str) -> Value {
    ignorarium::spec::load(Path::new(path)).unwrap().value
}

fn ctx(dir: &Path) -> CheckCtx {
    ctx_with(dir, Box::new(NoPriorUse))
}

fn ctx_with(dir: &Path, prior: Box<dyn PriorUse>) -> CheckCtx {
    CheckCtx {
        prior,
        records: Box::new(holding(pilot_estimates())),
        ..CheckCtx::empty(dir)
    }
}

/// The estimates of study `pilot`: a pooled one for each candidate of `ok()`'s selection
/// stage, as the pick, with variance 4 and icc 0.
fn pilot_estimates() -> Vec<Value> {
    vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(0.0)),
        common::estimate(&[("arm-b", 1.0, json!({ "bits": 4 }))], "", 4.0, Some(0.0)),
    ]
}

/// A box whose study `pilot` has finished with `estimates`.
fn holding(estimates: Vec<Value>) -> common::Studies {
    common::Studies(vec![(
        "pilot",
        Ok(common::records_for("pilot", &common::finished(estimates))),
    )])
}

/// `ok()`'s context with another box.
fn with_box(dir: &Path, b: common::Studies) -> CheckCtx {
    CheckCtx {
        records: Box::new(b),
        ..ctx(dir)
    }
}

fn named(spec: &Value, dir: &Path) -> Vec<Diagnostic> {
    rules::check_named(spec, v0(), &ctx(dir))
}

fn codes_paths(d: &[Diagnostic]) -> Vec<(&str, &str)> {
    d.iter()
        .map(|x| (x.code.as_str(), x.path.as_str()))
        .collect()
}

fn set(spec: &mut Value, ptr: &str, v: Option<Value>) {
    let (parent, key) = ptr.rsplit_once('/').unwrap();
    match (spec.pointer_mut(parent).unwrap(), v) {
        (Value::Object(m), Some(v)) => {
            m.insert(key.into(), v);
        }
        (Value::Object(m), None) => {
            m.remove(key).unwrap();
        }
        (Value::Array(a), Some(v)) => a[key.parse::<usize>().unwrap()] = v,
        _ => panic!("cannot apply {ptr}"),
    }
}

fn input(path: &str, role: &str, units: u64) -> Value {
    json!({
        "path": path, "sha256": "", "role": role, "stratum": "fiction", "units": units,
        "licence": { "spdx": "", "type": "unspecified", "source": "", "creators": [] }
    })
}

/// The template's calibration input, selection input `texts/s00.txt`, and `n` confirmation
/// inputs `texts/c<i>.txt`, each declaring `units` units. None of the files exist.
fn inputs(n: usize, units: u64) -> Value {
    let template = spec_at("../../templates/numeric-ablation.toml");
    let mut all = vec![
        template["inputs"][0].clone(),
        input("texts/s00.txt", "selection", units),
    ];
    all.extend((0..n).map(|i| input(&format!("texts/c{i:02}.txt"), "confirmation", units)));
    Value::Array(all)
}

/// The base plan's one hypothesis: the pick's paired value is below 0 in stratum `fiction`, by
/// the mean of each cluster, which lies in [-50, 50]. Power must hold at -30, with the sd and
/// icc of study `pilot`. It takes the whole level.
fn h1() -> Value {
    json!({
        "id": "H1", "statement": "", "rival": "", "outcome": "ppl-ratio",
        "contrast": { "selected": 1.0 }, "strata": ["fiction"],
        "claim": { "below": 0.0 }, "statistic": { "mean": [-50.0, 50.0] }, "power_at": -30.0,
        "estimates": { "from": "pilot" }, "alpha_share": 1.0
    })
}

/// Sizes `h1()` 1 below the margin, where the z floor decides: at sd 2, icc 0 and 5 units per
/// cluster a z-test needs 10 clusters. The narrower mean range keeps short the simulation that
/// follows a floor that passes.
fn near() -> Vec<Edit> {
    vec![
        hyp("power_at", j(json!(-1.0))),
        hyp("statistic", j(json!({ "mean": [-5.0, 5.0] }))),
    ]
}

/// `h1()` as `id`, with no claim: a descriptive hypothesis.
fn descriptive(id: &str) -> Value {
    let mut h = h1();
    h["id"] = json!(id);
    for k in ["claim", "statistic", "power_at", "estimates", "alpha_share"] {
        h.as_object_mut().unwrap().remove(k);
    }
    h
}

/// `h1()` as `H<i>` for each share, in order. The base plan's 35 clusters pass at each of these
/// shares: a share of 0.1 needs 31.
fn shared(shares: &[f64]) -> Vec<Edit> {
    let list: Vec<Value> = shares
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut h = h1();
            h["id"] = json!(format!("H{}", i + 1));
            h["alpha_share"] = json!(s);
            h
        })
        .collect();
    vec![("/question/hypotheses", j(json!(list)))]
}

/// Sets (or, with `None`, removes) a field of the first hypothesis.
fn hyp(field: &str, v: Option<Value>) -> Edit {
    let ptr = Box::leak(format!("/question/hypotheses/0/{field}").into_boxed_str());
    (ptr, v)
}

/// A confirmatory plan that passes every named check: 35 confirmation inputs of 5 units. At sd
/// 2 and icc 0 a cluster mean has sd 0.89; 30 below the margin, in a range of half-width 50,
/// each scores about -0.6, and the simulated test at the first stream level (0.005) and power
/// 0.8 needs 22 clusters (31 at a share of 0.1). A selection stage picks `arm-a` or `arm-b` on the selection
/// input, and the confirmation stage runs the pick, which `h1()` tests. Its code is not
/// `pilot`'s, and it declares a deviation from `pilot`.
fn ok() -> Value {
    let mut s = spec_at("../../templates/numeric-ablation.toml");
    let edits = [
        ("/question/hypotheses", json!([h1()])),
        (
            "/design/conditions",
            json!([{ "id": "exact" }, { "id": "arm-a" }, { "id": "arm-b", "params": { "bits": 4 } }]),
        ),
        (
            "/design/selection",
            json!({ "metric": "ppl-ratio", "conditions": ["arm-a", "arm-b"], "rule": "best-mean" }),
        ),
        (
            "/design/confirmation",
            json!({ "conditions": ["selected"] }),
        ),
        ("/study/mode", json!("confirmatory")),
        ("/design/units_per_cluster", json!(5)),
        ("/inputs", inputs(35, 5)),
        (
            "/mutations/triggers",
            json!([{ "id": "T1", "metric": "ppl-ratio", "threshold_e": 1.0 }]),
        ),
        ("/pins/deviations", json!([deviation("pilot")])),
    ];
    for (p, v) in edits {
        set(&mut s, p, Some(v));
    }
    s
}

type Edit = (&'static str, Option<Value>);

/// Removes the base plan's deviation.
const UNDECLARED: Edit = ("/pins/deviations", None);

/// A deviation from study `from`, made before the confirmation data were collected.
fn deviation(from: &str) -> Value {
    json!({
        "from": from, "reason": "The payload reads a newer tokenizer.",
        "timing": "before-collection"
    })
}

/// The model input `pilot` ran with (`common::instrument()`), as the plan declares it.
fn model_input() -> Value {
    json!({
        "path": "model/m.bin", "sha256": sha256_hex(common::MODEL), "role": "model",
        "licence": { "spdx": "", "type": "unspecified", "source": "", "creators": [] }
    })
}

/// `ok()` without its deviation, pinning `pilot`'s code: its payload (with the prefix), its
/// environment, and its one model input in place of the calibration input; then `extra`.
fn matched(extra: Vec<Edit>) -> Vec<Edit> {
    let i = common::instrument();
    and(
        vec![
            ("/pins/deviations", None),
            ("/pins/payload", j(json!(format!("sha256:{}", i.payload)))),
            ("/pins/environment", j(json!(i.environment))),
            ("/inputs/0", j(model_input())),
        ],
        extra,
    )
}

/// Edits on `ok()`, then the exact diagnostics (code, path) in rule-file order.
struct Fault {
    edits: Vec<Edit>,
    want: Vec<(&'static str, &'static str)>,
}

fn f(edits: Vec<Edit>, want: Vec<(&'static str, &'static str)>) -> Fault {
    Fault { edits, want }
}

fn j(v: Value) -> Option<Value> {
    Some(v)
}

/// Edits that turn on a follow-on route with these entries.
fn route(follow_on: Value) -> Vec<Edit> {
    vec![
        ("/scope/end_of_run/route", j(json!("follow-on"))),
        ("/scope/end_of_run/follow_on", j(follow_on)),
    ]
}

fn one(when: &str, spec: &str) -> Value {
    json!([{ "when": when, "spec": spec }])
}

/// `clusters` confirmation inputs that each give `upc` units.
fn power_edits(upc: u32, clusters: usize) -> Vec<Edit> {
    vec![
        ("/design/units_per_cluster", j(json!(upc))),
        ("/inputs", j(inputs(clusters, upc.into()))),
    ]
}

/// No selection stage: the confirmation stage runs `arm-a`, the hypothesis tests it, and
/// `extra` follows.
fn unstaged(extra: Vec<Edit>) -> Vec<Edit> {
    let mut e = vec![
        ("/design/selection", None),
        ("/design/confirmation/conditions", j(json!(["arm-a"]))),
        hyp("contrast", j(json!({ "arm-a": 1.0 }))),
    ];
    e.extend(extra);
    e
}

fn and(mut a: Vec<Edit>, b: Vec<Edit>) -> Vec<Edit> {
    a.extend(b);
    a
}

/// An exploratory plan with `h1()` and the descriptive `P1`, no deviation, and `extra`.
fn exploring(extra: Vec<Edit>) -> Vec<Edit> {
    and(
        vec![
            ("/study/mode", j(json!("exploratory"))),
            ("/question/hypotheses", j(json!([h1(), descriptive("P1")]))),
            UNDECLARED,
        ],
        extra,
    )
}

/// Two clusters of 5 units, 0.1 below the margin, and one edit: small enough that the gate
/// fires if it computes.
fn guard(ptr: &'static str, v: Value) -> Vec<Edit> {
    let mut e = power_edits(5, 2);
    e.push(hyp("power_at", j(json!(-0.1))));
    e.push((ptr, j(v)));
    e
}

const POWER: (&str, &str) = ("E-CNF-002", "question.hypotheses[0]");
/// The first hypothesis takes estimates from a study whose code the plan does not pin.
const SAME: (&str, &str) = ("E-REF-003", "question.hypotheses[0].estimates");
/// The first deviation names a study the plan takes no estimates from.
const STRAY: (&str, &str) = ("E-REF-003", "pins.deviations[0].from");
/// The start of the problem the simulation states.
const SIM: &str = "The confirmation sample is too small. Simulated power ";

fn faults() -> Vec<Fault> {
    // Built at run time: the leak gate rejects a drive prefix written in a source file.
    let drive: &'static str = Box::leak(format!("{}:{}", 'C', "/Windows/x.txt").into_boxed_str());
    let pth = |p: &'static str| {
        f(
            vec![("/inputs/0/path", j(json!(p)))],
            vec![("E-PTH-001", "inputs[0].path")],
        )
    };
    vec![
        f(
            vec![
                ("/study/mode", j(json!("exploratory"))),
                UNDECLARED,
                ("/study/label", j(json!("replicated"))),
            ],
            vec![("E-LBL-001", "study.label")],
        ),
        // power_at lies strictly on the claimed side, inside the mean range; a mistyped one is
        // the type rule's.
        f(
            vec![hyp("power_at", None)],
            vec![("E-CNF-001", "question.hypotheses[0].power_at")],
        ),
        f(
            vec![hyp("power_at", j(json!(0.0)))],
            vec![("E-CNF-001", "question.hypotheses[0].power_at")],
        ),
        f(
            vec![hyp("power_at", j(json!(-50.1)))],
            vec![("E-CNF-001", "question.hypotheses[0].power_at")],
        ),
        // At the end of the range E-CNF-001 accepts it; units below the range are clipped to
        // its end, so no cluster mean falls outside it.
        f(vec![hyp("power_at", j(json!(-50.0)))], vec![]),
        f(vec![hyp("power_at", j(json!("low")))], vec![]),
        // 9 clusters against the 22 the simulated test needs; 1 below the margin, 9 against
        // the z floor's 10.
        f(power_edits(5, 9), vec![POWER]),
        f(and(near(), power_edits(5, 9)), vec![POWER]),
        // Selection inputs do not count: 22 confirmation inputs pass, 21 do not.
        f(vec![("/inputs", j(inputs(21, 5)))], vec![POWER]),
        f(vec![("/inputs", j(inputs(22, 5)))], vec![]),
        // The sd and icc come from a finished study's estimates. A plan that takes none from
        // pilot declares a deviation from a study it takes no estimates from.
        f(
            vec![hyp("estimates", None)],
            vec![STRAY, ("E-CNF-003", "question.hypotheses[0].estimates")],
        ),
        f(
            vec![hyp("estimates", j(json!({ "from": "nobody" })))],
            vec![("W-REF-001", "question.hypotheses[0].estimates"), STRAY],
        ),
        f(
            vec![hyp("estimates", j(json!({ "from": "my-study" })))],
            vec![
                ("E-REF-002", "question.hypotheses[0].estimates.from"),
                STRAY,
            ],
        ),
        // A mistyped or malformed reference is the generic rules'; the deviation from pilot
        // then names a study the plan takes no estimates from.
        f(vec![hyp("estimates", j(json!("pilot")))], vec![STRAY]),
        f(vec![hyp("estimates", j(json!({})))], vec![STRAY]),
        f(
            vec![hyp("estimates", j(json!({ "from": "../pilot" })))],
            vec![STRAY],
        ),
        f(vec![hyp("estimates", j(json!({ "from": 3 })))], vec![STRAY]),
        // The plan pins the code of the study it takes estimates from, or declares a deviation
        // from it. `matched` pins pilot's code.
        f(matched(vec![]), vec![]),
        f(
            matched(vec![("/pins/payload", j(json!("ab".repeat(32))))]),
            vec![SAME],
        ),
        f(
            matched(vec![(
                "/pins/payload",
                j(json!(common::instrument().payload)),
            )]),
            vec![],
        ),
        f(
            matched(vec![(
                "/pins/payload",
                j(json!(format!(" sha256:{} ", common::instrument().payload))),
            )]),
            vec![],
        ),
        // An empty payload pin stands for the payload folder's hash; this root has no folder.
        f(matched(vec![("/pins/payload", j(json!("")))]), vec![SAME]),
        f(
            matched(vec![("/pins/environment", j(json!("")))]),
            vec![SAME],
        ),
        f(
            matched(vec![(
                "/pins/environment",
                j(json!(format!(
                    "sha256:{}",
                    common::instrument().environment
                ))),
            )]),
            vec![],
        ),
        f(
            matched(vec![("/inputs/0/role", j(json!("calibration")))]),
            vec![SAME],
        ),
        f(
            matched(vec![("/inputs/0/sha256", j(json!("")))]),
            vec![SAME],
        ),
        f(
            matched(vec![
                ("/pins/payload", j(json!(""))),
                ("/pins/deviations", j(json!([deviation("pilot")]))),
            ]),
            vec![],
        ),
        f(
            matched(vec![("/pins/deviations", j(json!([deviation("nobody")])))]),
            vec![STRAY],
        ),
        f(
            vec![("/pins/deviations/0/from", j(json!("nobody")))],
            vec![SAME, STRAY],
        ),
        f(
            vec![("/pins/deviations/0/from", j(json!("my-study")))],
            vec![SAME, STRAY],
        ),
        f(
            vec![(
                "/pins/deviations",
                j(json!([deviation("pilot"), deviation("nobody")])),
            )],
            vec![("E-REF-003", "pins.deviations[1].from")],
        ),
        // A deviation whose study is mistyped or no study id is the generic rules', and does
        // not cover pilot.
        f(vec![("/pins/deviations/0/from", j(json!(3)))], vec![SAME]),
        f(
            vec![("/pins/deviations/0/from", j(json!("Pilot")))],
            vec![SAME],
        ),
        // An exploratory plan takes no estimates, so it may declare no deviation, not even
        // from the study its hypothesis names.
        f(
            exploring(vec![("/pins/deviations", j(json!([deviation("pilot")])))]),
            vec![STRAY],
        ),
        // No confirmation input leaves the stratum with no data and the gate with no cluster.
        f(
            vec![("/inputs", j(inputs(0, 5)))],
            vec![
                ("E-CNF-004", "inputs"),
                ("E-HYP-002", "question.hypotheses[0].strata[0]"),
            ],
        ),
        // No selection or confirmation input: the schedule would be empty, in every mode.
        f(
            unstaged(vec![("/inputs", j(json!([inputs(0, 5)[0]])))]),
            vec![
                ("E-CNF-004", "inputs"),
                ("E-UNT-001", "inputs"),
                ("E-HYP-002", "question.hypotheses[0].strata[0]"),
            ],
        ),
        f(
            unstaged(vec![
                ("/study/mode", j(json!("exploratory"))),
                UNDECLARED,
                ("/inputs", j(json!([inputs(0, 5)[0]]))),
            ]),
            vec![
                ("E-UNT-001", "inputs"),
                ("E-HYP-002", "question.hypotheses[0].strata[0]"),
            ],
        ),
        // A selection or confirmation input declares its units; one without them is not counted.
        f(
            vec![("/inputs", j(inputs(36, 5))), ("/inputs/2/units", None)],
            vec![("E-UNT-001", "inputs[2].units")],
        ),
        f(
            vec![("/inputs/1/units", None)],
            vec![("E-UNT-001", "inputs[1].units")],
        ),
        // Calibration and model inputs need neither units nor a stratum.
        f(vec![("/inputs/0/role", j(json!("model")))], vec![]),
        // A stratum, when the design names strata.
        f(
            vec![("/inputs/2/stratum", j(json!("")))],
            vec![("E-UNT-001", "inputs[2].stratum")],
        ),
        f(
            vec![("/inputs/1/stratum", None)],
            vec![("E-UNT-001", "inputs[1].stratum")],
        ),
        f(
            vec![
                ("/design/stratum", j(json!(""))),
                ("/inputs/2/stratum", j(json!(""))),
            ],
            vec![],
        ),
        // 36 inputs of 50,001 units, each run twice or more: above the row cap.
        f(
            vec![
                ("/design/units_per_cluster", j(json!(50_001))),
                ("/inputs", j(inputs(35, 50_001))),
            ],
            vec![("E-UNT-002", "design.units_per_cluster")],
        ),
        f(
            vec![("/design/reference", j(json!("exakt")))],
            vec![("E-REF-001", "design.reference")],
        ),
        // The reference may be any condition. The pilot's values are paired with exact, so with
        // another reference none of its estimates is this contrast's. An empty condition list
        // is reported by non-empty alone.
        f(
            vec![
                ("/design/reference", j(json!("arm-a"))),
                ("/design/selection/conditions", j(json!(["exact", "arm-b"]))),
            ],
            vec![("E-CNF-003", "question.hypotheses[0].estimates")],
        ),
        f(vec![("/design/conditions", j(json!([])))], vec![]),
        f(
            vec![(
                "/design/conditions",
                j(
                    json!([{ "id": "exakt" }, 5, { "id": "arm-a" }, { "id": "arm-b", "params": { "bits": 4 } }]),
                ),
            )],
            vec![("E-REF-001", "design.reference")],
        ),
        // The stages: a stage needs a selection input, and a selection input a stage.
        f(
            vec![("/inputs/1/role", j(json!("confirmation")))],
            vec![("E-SEL-001", "design.selection")],
        ),
        f(unstaged(vec![]), vec![("E-SEL-001", "inputs[1].role")]),
        // Two or more candidates, each a declared condition other than the reference.
        f(
            vec![("/design/selection/conditions", j(json!(["arm-a"])))],
            vec![("E-SEL-001", "design.selection.conditions")],
        ),
        f(
            vec![("/design/selection/conditions", j(json!(["exact", "arm-a"])))],
            vec![("E-SEL-001", "design.selection.conditions[0]")],
        ),
        f(
            vec![("/design/selection/conditions", j(json!(["arm-a", "arm-z"])))],
            vec![("E-SEL-001", "design.selection.conditions[1]")],
        ),
        f(
            vec![(
                "/design/selection/conditions",
                j(json!(["arm-a", "selected"])),
            )],
            vec![("E-SEL-001", "design.selection.conditions[1]")],
        ),
        f(
            vec![("/design/selection/metric", j(json!("nope")))],
            vec![("E-SEL-001", "design.selection.metric")],
        ),
        // `selected` needs a stage, in the confirmation stage and in a contrast; no condition
        // may take its name.
        f(
            vec![
                ("/design/selection", None),
                ("/inputs/1/role", j(json!("confirmation"))),
            ],
            vec![
                ("E-SEL-001", "design.confirmation.conditions[0]"),
                ("E-HYP-001", "question.hypotheses[0].contrast.selected"),
            ],
        ),
        f(
            vec![(
                "/design/conditions",
                j(
                    json!([{ "id": "exact" }, { "id": "arm-a" }, { "id": "arm-b", "params": { "bits": 4 } }, { "id": "selected" }]),
                ),
            )],
            vec![("E-SEL-001", "design.conditions[3].id")],
        ),
        // The confirmation stage runs declared conditions and the pick, never the reference.
        f(
            vec![(
                "/design/confirmation/conditions",
                j(json!(["selected", "arm-z"])),
            )],
            vec![("E-SEL-001", "design.confirmation.conditions[1]")],
        ),
        f(
            vec![(
                "/design/confirmation/conditions",
                j(json!(["exact", "selected"])),
            )],
            vec![("E-SEL-001", "design.confirmation.conditions[0]")],
        ),
        f(
            vec![(
                "/design/confirmation/conditions",
                j(json!(["arm-a", "selected"])),
            )],
            vec![],
        ),
        // A param is a string or a number.
        f(
            vec![("/design/conditions/2/params/bits", j(json!([4])))],
            vec![("E-TYP-001", "design.conditions[2].params.bits")],
        ),
        // A reference to a study is a param too: pending while the study is not in the box.
        f(
            vec![(
                "/design/conditions/2/params/bits",
                j(json!({ "from": "x" })),
            )],
            vec![("W-REF-001", "design.conditions[2].params.bits")],
        ),
        f(
            vec![(
                "/design/conditions/2/params/bits",
                j(json!({ "from": "x", "to": "y" })),
            )],
            vec![("E-TYP-001", "design.conditions[2].params.bits")],
        ),
        f(
            vec![("/design/conditions/2/params/bits", j(json!({ "from": 3 })))],
            vec![("E-TYP-001", "design.conditions[2].params.bits")],
        ),
        // A reference is frozen under its key, so the key must be one a diagnostic can print.
        f(
            vec![(
                "/design/conditions/2/params/Pasted Key",
                j(json!({ "from": "x" })),
            )],
            vec![("E-TYP-001", "design.conditions[2].params.<key>")],
        ),
        f(
            vec![("/design/conditions/2/params/bits", j(json!(true)))],
            vec![("E-TYP-001", "design.conditions[2].params.bits")],
        ),
        f(
            vec![("/design/conditions/2/params/Pasted Key", j(json!([])))],
            vec![("E-TYP-001", "design.conditions[2].params.<key>")],
        ),
        // Other params are another contrast, which the pilot did not measure.
        f(
            vec![
                ("/design/conditions/2/params/bits", j(json!("four"))),
                ("/design/conditions/1/params", j(json!({ "rate": 0.5 }))),
            ],
            vec![("E-CNF-003", "question.hypotheses[0].estimates")],
        ),
        // A confirmatory hypothesis needs a claim; the rules that size and split a claim skip
        // it. Without a claim it takes no estimates, so the deviation from pilot is stray.
        f(
            vec![hyp("claim", None)],
            vec![STRAY, ("E-CNF-006", "question.hypotheses[0].claim")],
        ),
        // A hypothesis names a listed outcome, and a contrast with non-zero weights.
        f(
            vec![hyp("outcome", j(json!("nope")))],
            vec![("E-HYP-001", "question.hypotheses[0].outcome")],
        ),
        f(
            vec![hyp("contrast", j(json!({})))],
            vec![("E-HYP-001", "question.hypotheses[0].contrast")],
        ),
        f(
            vec![hyp("contrast", j(json!({ "selected": 0.0 })))],
            vec![("E-HYP-001", "question.hypotheses[0].contrast.selected")],
        ),
        // Each term is a condition the confirmation stage runs: not an undeclared one, not a
        // candidate only the selection stage runs, and not the reference, which has no paired
        // value.
        f(
            vec![hyp(
                "contrast",
                j(json!({ "selected": 1.0, "arm-z": -1.0 })),
            )],
            vec![("E-HYP-001", "question.hypotheses[0].contrast.arm-z")],
        ),
        f(
            vec![hyp(
                "contrast",
                j(json!({ "arm-a": 1.0, "selected": -1.0 })),
            )],
            vec![("E-HYP-001", "question.hypotheses[0].contrast.arm-a")],
        ),
        f(
            vec![hyp("contrast", j(json!({ "exact": 1.0 })))],
            vec![("E-HYP-001", "question.hypotheses[0].contrast.exact")],
        ),
        // A well-formed contrast that the pilot did not measure has no estimate.
        f(
            vec![
                (
                    "/design/confirmation/conditions",
                    j(json!(["arm-a", "selected"])),
                ),
                hyp("contrast", j(json!({ "arm-a": 1.0, "selected": -1.0 }))),
            ],
            vec![("E-CNF-003", "question.hypotheses[0].estimates")],
        ),
        // A key from the spec is printed only when safe; a key with a dot is known-key's alone.
        f(
            vec![hyp(
                "contrast",
                j(json!({ "selected": 1.0, "Pasted Key": 1.0 })),
            )],
            vec![("E-HYP-001", "question.hypotheses[0].contrast.<key>")],
        ),
        f(
            vec![hyp("contrast", j(json!({ "selected": 1.0, "a.b": 0.0 })))],
            vec![],
        ),
        // An undeclared condition that the stage lists is the stage's fault, reported once.
        f(
            vec![
                (
                    "/design/confirmation/conditions",
                    j(json!(["selected", "arm-z"])),
                ),
                hyp("contrast", j(json!({ "selected": 1.0, "arm-z": -1.0 }))),
            ],
            vec![("E-SEL-001", "design.confirmation.conditions[1]")],
        ),
        // A mistyped weight is the type rule's.
        f(
            vec![hyp("contrast", j(json!({ "selected": "one" })))],
            vec![],
        ),
        // Each listed stratum has a confirmation input; a selection input does not count.
        f(
            vec![hyp("strata", j(json!(["fiction", "code"])))],
            vec![("E-HYP-002", "question.hypotheses[0].strata[1]")],
        ),
        f(
            vec![
                ("/inputs/1/stratum", j(json!("code"))),
                hyp("strata", j(json!(["code"]))),
            ],
            vec![("E-HYP-002", "question.hypotheses[0].strata[0]")],
        ),
        f(vec![hyp("strata", j(json!([])))], vec![]),
        f(vec![hyp("strata", None)], vec![]),
        // A claim is one of below, above or within [a, b] with a < b.
        f(
            vec![hyp("claim", j(json!({ "below": 0.0, "above": -1.0 })))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(
            vec![hyp("claim", j(json!({})))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(
            vec![hyp("claim", j(json!({ "within": [2.0, 1.0] })))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(
            vec![hyp("claim", j(json!({ "within": [1.0, 1.0] })))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(
            vec![hyp("claim", j(json!({ "within": [1.0] })))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        // Exploratory, so no power_at must lie inside these claims.
        f(
            exploring(vec![hyp("claim", j(json!({ "within": [-2.0, 1.0] })))]),
            vec![],
        ),
        f(
            exploring(vec![hyp("claim", j(json!({ "above": -2.0 })))]),
            vec![],
        ),
        // A mean range is [lo, hi] with lo < hi, and every margin lies strictly inside it.
        f(
            vec![hyp("statistic", j(json!({ "mean": [5.0, 4.0] })))],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        f(
            vec![hyp("statistic", j(json!({ "mean": [5.0, 5.0] })))],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        f(
            vec![hyp("statistic", j(json!({ "mean": [-5.0, 0.0, 5.0] })))],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        f(
            vec![hyp("claim", j(json!({ "below": 50.0 })))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(
            vec![hyp("claim", j(json!({ "above": -50.0 })))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(
            vec![hyp("claim", j(json!({ "within": [-60.0, 0.0] })))],
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(vec![hyp("claim", j(json!({ "below": 49.9 })))], vec![]),
        // A bad range is reported once, not again as margins outside it.
        f(
            vec![
                hyp("statistic", j(json!({ "mean": [5.0, 4.0] }))),
                hyp("claim", j(json!({ "below": 100.0 }))),
            ],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        // A quantile lies strictly between 0 and 1, and takes any margin (exploratory: the
        // gate would ask for about 100 clusters, since a share of 1 scores only 1/9 at q 0.9).
        f(
            vec![hyp("statistic", j(json!({ "quantile": 1.0 })))],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        f(
            vec![hyp("statistic", j(json!({ "quantile": 0.0 })))],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        f(
            exploring(vec![
                hyp("statistic", j(json!({ "quantile": 0.9 }))),
                hyp("claim", j(json!({ "below": 500.0 }))),
            ]),
            vec![],
        ),
        f(
            vec![hyp(
                "statistic",
                j(json!({ "quantile": 0.5, "mean": [0, 1] })),
            )],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        // A claim needs its statistic.
        f(
            vec![hyp("statistic", None)],
            vec![("E-HYP-003", "question.hypotheses[0].statistic")],
        ),
        // Mistyped values and unknown keys are the generic rules'.
        f(vec![hyp("claim", j(json!({ "below": "x" })))], vec![]),
        f(vec![hyp("claim", j(json!({ "near": 100.0 })))], vec![]),
        f(vec![hyp("statistic", j(json!({ "median": 2.0 })))], vec![]),
        f(
            vec![hyp("statistic", j(json!({ "mean": ["a", 1] })))],
            vec![],
        ),
        f(vec![("/question/hypotheses", j(json!([1])))], vec![STRAY]),
        // With no outcome list or no confirmation list, the generic rules report that.
        f(
            vec![("/outcomes", None)],
            vec![("E-SEL-001", "design.selection.metric")],
        ),
        f(vec![("/design/confirmation", None)], vec![]),
        // Confirmatory shares lie in (0, 1] and sum to 1.
        f(
            vec![hyp("alpha_share", j(json!(0.0)))],
            vec![("E-HYP-004", "question.hypotheses[0].alpha_share")],
        ),
        f(
            vec![hyp("alpha_share", j(json!(1.5)))],
            vec![("E-HYP-004", "question.hypotheses[0].alpha_share")],
        ),
        f(
            vec![hyp("alpha_share", None)],
            vec![("E-HYP-004", "question.hypotheses[0].alpha_share")],
        ),
        f(
            shared(&[0.5, 0.4]),
            vec![("E-HYP-004", "question.hypotheses")],
        ),
        f(
            shared(&[0.5, 0.6]),
            vec![("E-HYP-004", "question.hypotheses")],
        ),
        // 0.7 + 0.2 + 0.1 sums to 0.9999999999999999: 1 within rounding.
        f(shared(&[0.7, 0.2, 0.1]), vec![]),
        f(shared(&[0.5, 0.5]), vec![]),
        // A share out of range is reported once, not again in the sum.
        f(
            shared(&[0.0, 1.0, 0.5]),
            vec![("E-HYP-004", "question.hypotheses[0].alpha_share")],
        ),
        f(vec![hyp("alpha_share", j(json!("half")))], vec![]),
        // A hypothesis with no claim takes no share, and a share it has does not count.
        f(
            vec![("/question/hypotheses", j(json!([h1(), descriptive("P1")])))],
            vec![("E-CNF-006", "question.hypotheses[1].claim")],
        ),
        f(
            vec![
                ("/question/hypotheses", j(json!([h1(), descriptive("P1")]))),
                ("/question/hypotheses/1/alpha_share", j(json!(0.5))),
            ],
            vec![("E-CNF-006", "question.hypotheses[1].claim")],
        ),
        // `.pass` and `.fail` need a hypothesis with a claim; a malformed claim is still one.
        f(
            exploring(route(one("P1.pass", "f.toml"))),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            and(
                route(one("H1.pass", "f.toml")),
                vec![hyp("claim", j(json!({})))],
            ),
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        f(
            vec![
                ("/scope/budget/cpu_hours", j(json!(0.0))),
                ("/scope/budget/wall_hours", j(json!(0))),
                ("/scope/budget/money_usd", j(json!(0.0))),
            ],
            vec![("E-BUD-001", "scope.budget")],
        ),
        f(
            vec![
                ("/scope/budget/cpu_hours", None),
                ("/scope/budget/wall_hours", None),
                ("/scope/budget/money_usd", None),
            ],
            vec![("E-BUD-001", "scope.budget")],
        ),
        f(
            route(json!([])),
            vec![("E-END-001", "scope.end_of_run.follow_on")],
        ),
        f(
            route(one("H9.pass", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one("H1.maybe", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one("H1", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one(".pass", "f.toml")),
            vec![("E-END-001", "scope.end_of_run.follow_on[0].when")],
        ),
        f(
            route(one("H1.fail", "missing.toml")),
            vec![("E-END-002", "scope.end_of_run.follow_on[0].spec")],
        ),
        f(
            vec![("/mutations/depth_cap", j(json!(3)))],
            vec![("E-MUT-001", "mutations.depth_cap")],
        ),
        f(
            vec![("/mutations/seeds/report", j(json!("spawn/")))],
            vec![("E-MUT-002", "mutations.seeds")],
        ),
        f(
            vec![("/mutations/seeds/spawn", j(json!("report/x")))],
            vec![("E-MUT-002", "mutations.seeds")],
        ),
        f(
            vec![("/mutations/seeds/report", j(json!("spawn/x")))],
            vec![("E-MUT-002", "mutations.seeds")],
        ),
        f(
            vec![("/mutations/triggers/0/metric", j(json!("nope")))],
            vec![("E-MUT-003", "mutations.triggers[0].metric")],
        ),
        // machine_max_level is 2.
        f(
            vec![("/scope/min_isolation", j(json!(3)))],
            vec![("E-ISO-001", "scope.min_isolation")],
        ),
        pth("../secret.txt"),
        pth("data/../../x.txt"),
        pth("/etc/passwd"),
        pth(drive),
        pth("E:data.txt"),
        pth("data\\x.txt"),
        pth(""),
        f(
            route(one("H1.pass", "../next.toml")),
            vec![("E-PTH-001", "scope.end_of_run.follow_on[0].spec")],
        ),
        // An unsafe path is never read: no hash or freshness check follows it.
        f(
            vec![
                ("/inputs/0/path", j(json!("../secret.txt"))),
                ("/inputs/0/sha256", j(json!("b".repeat(64)))),
                ("/inputs/0/role", j(json!("confirmation"))),
                ("/inputs/0/units", j(json!(5))),
                ("/inputs/0/stratum", j(json!("fiction"))),
            ],
            vec![("E-PTH-001", "inputs[0].path")],
        ),
        // Overflow: the product of user integers saturates; the plan then passes. A units per
        // cluster below 1 is the range rule's.
        f(
            vec![("/design/units_per_cluster", j(json!(i64::MAX)))],
            vec![],
        ),
        f(vec![("/design/units_per_cluster", j(json!(-5)))], vec![]),
        f(vec![("/design/units_per_cluster", j(json!(0)))], vec![]),
        // Power-gate guards: with 2 clusters the gate fires if it computes, so [] means it skipped.
        f(guard("/design/power", json!(0.0)), vec![]),
        f(guard("/design/power", json!(1.0)), vec![]),
        f(guard("/design/power", json!("x")), vec![]),
        f(guard("/design/power", json!(0.999)), vec![POWER]),
        f(guard("/design/power", json!(0.5)), vec![POWER]),
        f(
            guard("/question/hypotheses/0/alpha_share", json!(1.0)),
            vec![POWER],
        ),
        // A share out of range and a claim that is not well formed skip the gate; their own
        // rules report them.
        f(
            guard("/question/hypotheses/0/alpha_share", json!(0.0)),
            vec![("E-HYP-004", "question.hypotheses[0].alpha_share")],
        ),
        f(
            guard("/question/hypotheses/0/claim", json!({})),
            vec![("E-HYP-003", "question.hypotheses[0].claim")],
        ),
        // Fail closed: an absent power cannot be computed.
        f(vec![("/design/power", None)], vec![POWER]),
        // An empty array as an element is no input, and no cluster.
        f(
            vec![(
                "/inputs",
                j(json!(
                    inputs(35, 5)
                        .as_array()
                        .unwrap()
                        .iter()
                        .chain(&[json!([])])
                        .collect::<Vec<_>>()
                )),
            )],
            vec![],
        ),
        f(route(json!([{ "when": [], "spec": "f.toml" }])), vec![]),
        // A budget of negatives is not a budget.
        f(
            vec![
                ("/scope/budget/cpu_hours", j(json!(-1.0))),
                ("/scope/budget/wall_hours", j(json!(0.0))),
                ("/scope/budget/money_usd", j(json!(0.0))),
            ],
            vec![("E-BUD-001", "scope.budget")],
        ),
        // Path forms: a lone `..`, a colon, and a segment Windows would trim.
        pth(".."),
        pth("a/b:c"),
        pth("a. /x"),
        pth("a./x"),
        pth("a/."),
        f(
            route(one("H1.pass", "..")),
            vec![("E-PTH-001", "scope.end_of_run.follow_on[0].spec")],
        ),
    ]
}

/// The study folder every table row runs in: `f.toml` and `pilot`'s model file
/// `model/m.bin` exist, `data/example.txt` does not.
fn study_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.toml"), "x = 1\n").unwrap();
    write(dir.path(), "model/m.bin", common::MODEL);
    dir
}

fn edited(edits: &[Edit]) -> Value {
    let mut s = ok();
    for (p, v) in edits {
        set(&mut s, p, v.clone());
    }
    s
}

#[test]
fn an_empty_box_has_no_study_and_no_closed_slot() {
    let empty = rules::NoRecords;
    assert_eq!(empty.study("pilot"), Ok(None));
    assert_eq!(empty.stream_closed("planlog", "pilot", 1), Ok(false));
    for log in ["planlog", "testlog"] {
        let next = empty.stream_next(log).unwrap();
        assert_eq!(next.slot, 1, "{log}");
        assert!((next.level - 0.005).abs() < 1e-12, "{log}: {}", next.level);
    }
    assert!(empty.stream_next("other").is_err());
}

#[test]
fn checks_read_files_from_the_context_root() {
    let dir = study_dir();
    let other = tempfile::tempdir().unwrap();
    assert_eq!(CheckCtx::empty(dir.path()).root, dir.path());
    let spec = edited(&route(one("H1.fail", "f.toml")));
    assert!(
        !codes_paths(&named(&spec, dir.path()))
            .contains(&("E-END-002", "scope.end_of_run.follow_on[0].spec"))
    );
    assert!(
        codes_paths(&named(&spec, other.path()))
            .contains(&("E-END-002", "scope.end_of_run.follow_on[0].spec"))
    );
}

#[test]
fn the_base_plan_passes_every_named_check() {
    let dir = study_dir();
    assert!(named(&ok(), dir.path()).is_empty());
}

/// `estimate_inputs` of `ok()`'s hypothesis, edited, against a box whose `pilot` holds
/// `estimates`.
fn inputs_of(edits: &[Edit], estimates: Vec<Value>) -> Lookup {
    let dir = study_dir();
    estimate_inputs(&edited(edits), 0, &with_box(dir.path(), holding(estimates)))
}

/// The hash of each record of `pilot` with `estimates`, in order: register, analysis, then
/// one per estimate.
fn hashes(estimates: Vec<Value>) -> Vec<String> {
    common::records_for("pilot", &common::finished(estimates))
        .iter()
        .map(records::hash)
        .collect()
}

fn stratum(name: &str, sd: f64, icc: f64, records: &[&String]) -> StratumInput {
    StratumInput {
        stratum: name.into(),
        sd,
        icc,
        pooled: false,
        records: records.iter().map(|h| h.to_string()).collect(),
    }
}

/// The E-CNF-003 diagnostics, as (code and path, problem), for `ok()` edited, against `b`.
fn missing(edits: &[Edit], b: common::Studies) -> Vec<(String, String)> {
    let dir = study_dir();
    rules::check_named(&edited(edits), v0(), &with_box(dir.path(), b))
        .into_iter()
        .filter(|d| d.code == "E-CNF-003")
        .map(|d| (format!("{} {}", d.code, d.path), d.problem))
        .collect()
}

const ESTIMATES: &str = "E-CNF-003 question.hypotheses[0].estimates";
const NO_MATCH: &str =
    "A confirmatory hypothesis takes its sd and icc from no matching estimate of a finished study.";

#[test]
fn the_base_plan_reads_each_candidates_estimate() {
    let h = hashes(pilot_estimates());
    assert_eq!(
        inputs_of(&[], pilot_estimates()),
        Lookup::Found(vec![StratumInput {
            pooled: true,
            ..stratum("fiction", 2.0, 0.0, &[&h[2], &h[3]])
        }])
    );
}

#[test]
fn a_finished_study_without_a_match_is_refused() {
    let arm = |id: &str, w: f64, params: Value, stratum: &str| {
        common::estimate(&[(id, w, params)], stratum, 4.0, Some(0.0))
    };
    let a = arm("arm-a", 1.0, json!({}), "");
    let b = arm("arm-b", 1.0, json!({ "bits": 4 }), "");
    let detail = |pick: &str| {
        format!(
            "{NO_MATCH} Study pilot has no estimate of this contrast with {pick} as the pick in stratum fiction or the pooled group."
        )
    };
    // Another weight, other params, another stratum: each is not this contrast's estimate.
    for (held, pick) in [
        (vec![a.clone()], "arm-b"),
        (vec![b.clone()], "arm-a"),
        (vec![arm("arm-a", 2.0, json!({}), ""), b.clone()], "arm-a"),
        (
            vec![a.clone(), arm("arm-b", 1.0, json!({ "bits": 8 }), "")],
            "arm-b",
        ),
        (
            vec![a.clone(), arm("arm-b", 1.0, json!({ "bits": 4 }), "code")],
            "arm-b",
        ),
    ] {
        assert_eq!(
            missing(&[], holding(held)),
            [(ESTIMATES.to_string(), detail(pick))]
        );
    }
    // Without a selection stage the contrast is the one condition; pooled, the pooled group.
    let alone = unstaged(vec![hyp("strata", j(json!([])))]);
    assert_eq!(
        missing(&alone, holding(vec![b.clone()])),
        [(
            ESTIMATES.to_string(),
            format!("{NO_MATCH} Study pilot has no estimate of this contrast in the pooled group.")
        )]
    );
    assert!(missing(&alone, holding(vec![a.clone()])).is_empty());
    // Another measure or paired rule is another key, too.
    let mut other = a.clone();
    other["key_hash"] = json!("0".repeat(64));
    assert_eq!(missing(&alone, holding(vec![other])).len(), 1);
    // No study named at all.
    assert_eq!(
        missing(&[hyp("estimates", None)], holding(vec![a, b])),
        [(
            ESTIMATES.to_string(),
            format!("{NO_MATCH} The hypothesis names no study.")
        )]
    );
}

#[test]
fn an_unreadable_box_fails_closed() {
    let torn = common::Studies(vec![("pilot", Err("line 2 is not a whole record".into()))]);
    assert_eq!(
        missing(&[], torn),
        [(
            ESTIMATES.to_string(),
            format!(
                "{NO_MATCH} The box cannot read the records of study pilot: line 2 is not a whole record."
            )
        )]
    );
}

#[test]
fn selected_takes_the_worst_candidate_estimate() {
    // arm-a has the larger icc, arm-b the larger sd: the gate takes both.
    let held = vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(0.3)),
        common::estimate(&[("arm-b", 1.0, json!({ "bits": 4 }))], "", 9.0, Some(0.1)),
    ];
    let h = hashes(held.clone());
    assert_eq!(
        inputs_of(&[], held.clone()),
        Lookup::Found(vec![StratumInput {
            pooled: true,
            ..stratum("fiction", 3.0, 0.3, &[&h[2], &h[3]])
        }])
    );
    // The other way round: arm-a has the larger sd, arm-b the larger icc.
    let swapped = vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 9.0, Some(0.1)),
        common::estimate(&[("arm-b", 1.0, json!({ "bits": 4 }))], "", 4.0, Some(0.3)),
    ];
    let s = hashes(swapped.clone());
    assert_eq!(
        inputs_of(&[], swapped),
        Lookup::Found(vec![StratumInput {
            pooled: true,
            ..stratum("fiction", 3.0, 0.3, &[&s[2], &s[3]])
        }])
    );
    // At sd 3 and icc 0.3 with 5 units per cluster, 1 below the margin, the gate needs 47.
    let dir = study_dir();
    let spec = edited(&and(near(), power_edits(5, 11)));
    let d = rules::check_named(&spec, v0(), &with_box(dir.path(), holding(held)));
    assert_eq!(
        d.iter().map(|x| x.problem.as_str()).collect::<Vec<_>>(),
        [
            "The confirmation sample is too small. Stratum fiction has 11 of the 47 clusters the power gate needs."
        ]
    );
}

#[test]
fn a_stratum_falls_back_to_the_pooled_estimate() {
    let a = |stratum: &str, var: f64| {
        common::estimate(&[("arm-a", 1.0, json!({}))], stratum, var, Some(0.1))
    };
    let held = vec![a("", 4.0), a("fiction", 9.0), a("code", 16.0)];
    let h = hashes(held.clone());
    let edits = unstaged(vec![hyp("strata", j(json!(["fiction", "article"])))]);
    assert_eq!(
        inputs_of(&edits, held.clone()),
        Lookup::Found(vec![
            stratum("fiction", 3.0, 0.1, &[&h[3]]),
            StratumInput {
                pooled: true,
                ..stratum("article", 2.0, 0.1, &[&h[2]])
            },
        ])
    );
    // No strata: the pooled group, and only its estimate.
    let pooled = unstaged(vec![hyp("strata", j(json!([])))]);
    assert_eq!(
        inputs_of(&pooled, held),
        Lookup::Found(vec![stratum("", 2.0, 0.1, &[&h[2]])])
    );
}

#[test]
fn an_estimate_without_icc_is_refused() {
    let pooled = unstaged(vec![hyp("strata", j(json!([])))]);
    let a = |var: f64, icc: Option<f64>| {
        holding(vec![common::estimate(
            &[("arm-a", 1.0, json!({}))],
            "",
            var,
            icc,
        )])
    };
    let detail = format!(
        "{NO_MATCH} Study pilot's estimate of this contrast in the pooled group has no icc, or no variance above 0."
    );
    for b in [a(4.0, None), a(0.0, Some(0.1))] {
        assert_eq!(
            missing(&pooled, b),
            [(ESTIMATES.to_string(), detail.clone())]
        );
    }
    assert!(missing(&pooled, a(4.0, Some(0.0))).is_empty());
    assert!(missing(&pooled, a(1e-9, Some(1.0))).is_empty());
    // Only an unusable estimate in the stratum and in the pool: the stratum is named.
    let held = vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "fiction", 4.0, None),
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, None),
    ];
    assert_eq!(
        missing(&unstaged(vec![]), holding(held)),
        [(
            ESTIMATES.to_string(),
            format!(
                "{NO_MATCH} Study pilot's estimate of this contrast in stratum fiction has no icc, or no variance above 0."
            )
        )]
    );
}

#[test]
fn a_stratum_estimate_without_icc_falls_back_to_the_pooled_one() {
    // One cluster in the stratum: no icc there. The pool has one.
    let held = vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "fiction", 9.0, None),
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(0.1)),
    ];
    let h = hashes(held.clone());
    let want = StratumInput {
        pooled: true,
        ..stratum("fiction", 2.0, 0.1, &[&h[3]])
    };
    assert_eq!(
        inputs_of(&unstaged(vec![]), held.clone()),
        Lookup::Found(vec![want])
    );
    assert!(missing(&unstaged(vec![]), holding(held)).is_empty());
    // A zero variance in the stratum is skipped the same way.
    let held = vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "fiction", 0.0, Some(0.2)),
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(0.1)),
    ];
    let h = hashes(held.clone());
    let got = inputs_of(&unstaged(vec![]), held);
    let want = StratumInput {
        pooled: true,
        ..stratum("fiction", 2.0, 0.1, &[&h[3]])
    };
    assert_eq!(got, Lookup::Found(vec![want]));
    // A usable stratum estimate is used as is.
    let held = vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "fiction", 9.0, Some(0.3)),
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(0.1)),
    ];
    let h = hashes(held.clone());
    assert_eq!(
        inputs_of(&unstaged(vec![]), held),
        Lookup::Found(vec![stratum("fiction", 3.0, 0.3, &[&h[2]])])
    );
}

#[test]
fn the_repeats_are_part_of_the_estimate_key() {
    let held = |repeats: u64| {
        let terms = [("arm-a".to_string(), 1.0, json!({}))];
        let reference = ("exact".to_string(), json!({}));
        let (key, key_hash) =
            ignorarium::derived::contrast_key(&common::ppl_ratio(), repeats, &reference, &terms);
        let mut e = common::arm_a();
        e["key"] = key;
        e["key_hash"] = json!(key_hash);
        holding(vec![e])
    };
    let pooled = unstaged(vec![hyp("strata", j(json!([])))]);
    let with = |repeats: u64| and(pooled.clone(), vec![("/design/repeats", j(json!(repeats)))]);
    assert!(missing(&with(3), held(3)).is_empty());
    assert_eq!(missing(&with(3), held(1)).len(), 1);
    assert_eq!(missing(&with(1), held(3)).len(), 1);
}

#[test]
fn an_unfinished_study_is_pending() {
    let detail = |which: &str| {
        format!(
            "A value waits on a study that has not finished. Waits on study pilot, which is {which}."
        )
    };
    let finished = common::finished(pilot_estimates());
    let unfinished = [
        (common::Studies(vec![]), "not registered"),
        (
            common::Studies(vec![(
                "pilot",
                Ok(common::records_for("pilot", &finished[..1])),
            )]),
            "not finished",
        ),
        // The analysis record promises two estimates, and one is there.
        (
            common::Studies(vec![(
                "pilot",
                Ok(common::records_for("pilot", &finished[..3])),
            )]),
            "not finished",
        ),
    ];
    let dir = study_dir();
    for (b, which) in unfinished {
        let d = rules::check_named(&ok(), v0(), &with_box(dir.path(), b));
        assert_eq!(
            codes_paths(&d),
            [("W-REF-001", "question.hypotheses[0].estimates")],
            "{which}"
        );
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].problem, detail(which));
    }
    assert_eq!(
        inputs_of(
            &[hyp("estimates", j(json!({ "from": "later" })))],
            pilot_estimates()
        ),
        Lookup::Pending("Waits on study later, which is not registered.".into())
    );
}

#[test]
fn a_reference_to_itself_is_refused() {
    let dir = study_dir();
    let d = named(
        &edited(&[hyp("estimates", j(json!({ "from": "my-study" })))]),
        dir.path(),
    );
    assert_eq!(
        codes_paths(&d),
        [
            ("E-REF-002", "question.hypotheses[0].estimates.from"),
            STRAY
        ]
    );
    assert_eq!(
        d[0].problem,
        "A reference names this study, or a study that recorded no selection."
    );
    // In a study with another id, the same reference names a study that has not run.
    let mut s = edited(&[hyp("estimates", j(json!({ "from": "my-study" })))]);
    s["study"]["id"] = json!("other-study");
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("W-REF-001", "question.hypotheses[0].estimates"), STRAY]
    );
}

#[test]
fn power_at_must_be_on_the_claimed_side() {
    let dir = study_dir();
    let at = |claim: Value, statistic: Value, power_at: f64| {
        let s = edited(&[
            hyp("claim", j(claim)),
            hyp("statistic", j(statistic)),
            hyp("power_at", j(json!(power_at))),
        ]);
        codes_paths(&named(&s, dir.path()))
            .iter()
            .any(|(c, _)| *c == "E-CNF-001")
    };
    let mean = json!({ "mean": [-50.0, 50.0] });
    let median = json!({ "quantile": 0.5 });
    for (claim, power_at, refused) in [
        (json!({ "below": 4.0 }), 5.0, true),
        (json!({ "below": 4.0 }), 4.0, true),
        (json!({ "below": 4.0 }), 2.0, false),
        (json!({ "above": 4.0 }), 4.0, true),
        (json!({ "above": 4.0 }), 2.0, true),
        (json!({ "above": 4.0 }), 6.0, false),
        (json!({ "within": [-1.0, 1.0] }), -1.0, true),
        (json!({ "within": [-1.0, 1.0] }), 1.0, true),
        (json!({ "within": [-1.0, 1.0] }), -2.0, true),
        (json!({ "within": [-1.0, 1.0] }), 0.0, false),
        // Inside the range of a cluster mean, ends included; a quantile has no range.
        (json!({ "below": 0.0 }), -50.0, false),
        (json!({ "below": 0.0 }), -50.1, true),
        (json!({ "above": 0.0 }), 50.0, false),
        (json!({ "above": 0.0 }), 50.1, true),
    ] {
        assert_eq!(
            at(claim.clone(), mean.clone(), power_at),
            refused,
            "{claim} at {power_at}"
        );
    }
    assert!(!at(json!({ "below": 0.0 }), median.clone(), -500.0));
    assert!(at(json!({ "below": 0.0 }), median, 0.0));
    let rule = v0().iter().find(|r| r.code == "E-CNF-001").unwrap();
    assert_eq!(
        rule.problem,
        "A confirmatory hypothesis has no power_at strictly on its claimed side, or power_at lies outside the range of its mean statistic."
    );
}

#[test]
fn each_stratum_needs_its_own_clusters() {
    // 26 clusters of one unit in each of two strata. At sd 2 the gate needs 47 per stratum.
    let mut list = inputs(0, 1).as_array().unwrap().clone();
    for (i, stratum) in (0..52).map(|i| (i, if i < 26 { "code" } else { "article" })) {
        let mut c = input(&format!("texts/c{i:02}.txt"), "confirmation", 1);
        c["stratum"] = json!(stratum);
        list.push(c);
    }
    list[1]["stratum"] = json!("code");
    let base = [
        ("/inputs", j(Value::Array(list))),
        ("/design/units_per_cluster", j(json!(1))),
    ];
    let dir = study_dir();
    let run = |strata: Value| {
        let mut e = and(near(), base.to_vec());
        e.push(hyp("strata", j(strata)));
        named(&edited(&e), dir.path())
            .into_iter()
            .map(|d| (d.path, d.problem))
            .collect::<Vec<_>>()
    };
    let short = |s: &str| {
        (
            "question.hypotheses[0]".to_string(),
            format!(
                "The confirmation sample is too small. Stratum {s} has 26 of the 47 clusters the power gate needs."
            ),
        )
    };
    assert_eq!(
        run(json!(["code", "article"])),
        [short("code"), short("article")]
    );
    assert_eq!(run(json!(["article"])), [short("article")]);
    // Pooled, the same 52 clusters pass the z floor, and the gate simulates.
    let pooled = run(json!([]));
    assert_eq!(pooled.len(), 1);
    assert!(pooled[0].1.starts_with(SIM), "{pooled:?}");
}

#[test]
fn two_confirmation_inputs_with_the_same_data_count_once() {
    // 22 clusters pass where 22 are needed; a copy of one, in other case and spacing, is the
    // same cluster.
    let dir = study_dir();
    write(dir.path(), "texts/s00.txt", b"selection text");
    for i in 0..22 {
        write(
            dir.path(),
            &format!("texts/c{i:02}.txt"),
            format!("text {i}").as_bytes(),
        );
    }
    let spec = edited(&power_edits(5, 22));
    assert!(named(&spec, dir.path()).is_empty());
    write(dir.path(), "texts/c21.txt", b"TEXT   20");
    assert_eq!(
        named(&spec, dir.path())
            .iter()
            .map(|d| d.problem.as_str())
            .collect::<Vec<_>>(),
        [format!(
            "{SIM}0.00 with 21 clusters in fiction; about 22 per stratum reach 0.80."
        )]
    );
}

#[test]
fn a_copy_in_another_stratum_is_not_a_cluster_of_that_stratum() {
    // 26 code texts and 26 article texts, but the first article text is a copy of the first
    // code text. The analysis keeps the earlier input of a same-data pair, so article has 25.
    let mut list = inputs(0, 1).as_array().unwrap().clone();
    list[1]["stratum"] = json!("code");
    let dir = study_dir();
    for i in 0..52 {
        let stratum = if i < 26 { "code" } else { "article" };
        let path = format!("texts/c{i:02}.txt");
        let text = if i == 26 { 0 } else { i };
        write(dir.path(), &path, format!("text {text}").as_bytes());
        let mut c = input(&path, "confirmation", 1);
        c["stratum"] = json!(stratum);
        list.push(c);
    }
    let mut e = and(
        near(),
        vec![
            ("/inputs", j(Value::Array(list))),
            ("/design/units_per_cluster", j(json!(1))),
        ],
    );
    e.push(hyp("strata", j(json!(["code", "article"]))));
    let got: Vec<String> = (named(&edited(&e), dir.path()).into_iter())
        .map(|d| d.problem)
        .collect();
    let short = |s: &str, n: u64| {
        format!(
            "The confirmation sample is too small. Stratum {s} has {n} of the 47 clusters the power gate needs."
        )
    };
    assert_eq!(got, [short("code", 26), short("article", 25)]);
}

#[test]
fn the_power_gate_uses_the_harmonic_mean_of_the_picked_units_per_cluster() {
    // 11 clusters of 5 units; k of them hold 1 unit. The estimate is the unweighted mean of
    // the cluster means, so a cluster mean has variance sd^2 (icc + (1 - icc) / m) on average
    // over 1 / m: the harmonic mean size. 1 small cluster: 11 / (10 / 5 + 1) = 3.67 units, 13
    // clusters needed; 2: 2.89 units, 17. The arithmetic mean, 4.6 and 4.3, would pass at 11.
    let dir = study_dir();
    let problems = |list: Value| {
        let e = and(near(), vec![("/inputs", j(list))]);
        named(&edited(&e), dir.path())
            .into_iter()
            .map(|d| d.problem)
            .collect::<Vec<_>>()
    };
    let with_small = |k: usize| {
        let mut list = inputs(11, 5);
        for i in 0..k {
            list[2 + i]["units"] = json!(1);
        }
        problems(list)
    };
    // 11 clusters of 5 pass the floor; the simulation follows.
    let none = with_small(0);
    assert!(none.len() == 1 && none[0].starts_with(SIM), "{none:?}");
    let short = |need: u64| {
        format!(
            "The confirmation sample is too small. Stratum fiction has 11 of the {need} clusters the power gate needs."
        )
    };
    assert_eq!(with_small(1), [short(13)]);
    assert_eq!(with_small(2), [short(17)]);
    // An input with 0 units is no cluster: the range rule reports it.
    let mut list = inputs(10, 5);
    list[2]["units"] = json!(0);
    assert_eq!(
        problems(list),
        [
            "The confirmation sample is too small. Stratum fiction has 9 of the 10 clusters the power gate needs."
        ]
    );
    // An input gives at most units_per_cluster: 9 units count as 5, so 9 clusters fall short.
    assert_eq!(
        problems(inputs(9, 9)),
        [
            "The confirmation sample is too small. Stratum fiction has 9 of the 10 clusters the power gate needs."
        ]
    );
}

#[test]
fn an_estimate_written_before_the_analysis_record_is_not_read() {
    // A stray estimate of arm-a with variance 1, then the analysis record and the one it
    // promises, with variance 4. Read, the stray one would size the study too small.
    let early = common::estimate(&[("arm-a", 1.0, json!({}))], "", 1.0, Some(0.0));
    let finished = common::finished(vec![common::estimate(
        &[("arm-a", 1.0, json!({}))],
        "",
        4.0,
        Some(0.0),
    )]);
    let mut bodies = vec![finished[0].clone(), ("estimate", early)];
    bodies.extend(finished[1..].iter().cloned());
    let records = common::records_for("pilot", &bodies);
    let h: Vec<String> = records.iter().map(records::hash).collect();
    let dir = study_dir();
    let b = common::Studies(vec![("pilot", Ok(records))]);
    let pooled = edited(&unstaged(vec![hyp("strata", j(json!([])))]));
    assert_eq!(
        estimate_inputs(&pooled, 0, &with_box(dir.path(), b)),
        Lookup::Found(vec![stratum("", 2.0, 0.0, &[&h[3]])])
    );
}

#[test]
fn two_estimates_of_one_contrast_must_agree() {
    let a = |hypothesis: &str, var: f64, icc: f64| {
        let mut e = common::estimate(&[("arm-a", 1.0, json!({}))], "", var, Some(icc));
        e["hypothesis"] = json!(hypothesis);
        e
    };
    let pooled = unstaged(vec![hyp("strata", j(json!([])))]);
    // Two hypotheses of the pilot on one contrast: the same values, both records frozen.
    let same = vec![a("H1", 4.0, 0.1), a("H2", 4.0, 0.1)];
    let h = hashes(same.clone());
    assert_eq!(
        inputs_of(&pooled, same),
        Lookup::Found(vec![stratum("", 2.0, 0.1, &[&h[2], &h[3]])])
    );
    // Other values for one key: no one estimate to take.
    let detail = format!(
        "{NO_MATCH} Study pilot holds 2 estimates of this contrast in the pooled group, with different variances or iccs."
    );
    for held in [
        vec![a("H1", 4.0, 0.1), a("H2", 9.0, 0.1)],
        vec![a("H1", 4.0, 0.1), a("H2", 4.0, 0.2)],
    ] {
        assert_eq!(
            missing(&pooled, holding(held)),
            [(ESTIMATES.to_string(), detail.clone())]
        );
    }
}

#[test]
fn the_reference_is_part_of_the_contrast() {
    // The plan's reference runs with other params than the pilot's: other paired values.
    let params = ("/design/conditions/0/params", j(json!({ "bits": 16 })));
    let pooled = unstaged(vec![hyp("strata", j(json!([]))), params.clone()]);
    assert_eq!(
        missing(&pooled, holding(pilot_estimates())),
        [(
            ESTIMATES.to_string(),
            format!("{NO_MATCH} Study pilot has no estimate of this contrast in the pooled group.")
        )]
    );
    let against = common::estimate_against(
        ("exact", json!({ "bits": 16 })),
        &[("arm-a", 1.0, json!({}))],
        "",
        4.0,
        Some(0.0),
    );
    assert!(missing(&pooled, holding(vec![against])).is_empty());
    let rule = v0().iter().find(|r| r.code == "E-CNF-003").unwrap();
    assert!(
        rule.instruction
            .contains("the same reference condition with the same params"),
        "{}",
        rule.instruction
    );
}

#[test]
fn a_negative_icc_counts_as_zero() {
    // An analysis of variance can estimate an icc below 0; the gate takes 0, the larger value.
    let pooled = unstaged(vec![hyp("strata", j(json!([])))]);
    let held = vec![common::estimate(
        &[("arm-a", 1.0, json!({}))],
        "",
        4.0,
        Some(-0.1),
    )];
    let h = hashes(held.clone());
    assert_eq!(
        inputs_of(&pooled, held),
        Lookup::Found(vec![stratum("", 2.0, 0.0, &[&h[2]])])
    );
}

#[test]
fn the_gate_takes_the_largest_need_over_the_bounds() {
    let dir = study_dir();
    let within = |power_at: f64| {
        let mut e = and(near(), power_edits(5, 11));
        e.push(hyp("claim", j(json!({ "within": [-2.0, 2.0] }))));
        e.push(hyp("power_at", j(json!(power_at))));
        named(&edited(&e), dir.path())
            .into_iter()
            .map(|d| d.problem)
            .collect::<Vec<_>>()
    };
    // 2 from each margin needs 3 clusters, so the simulation follows; 0.5 from the upper
    // margin needs 38.
    let far = within(0.0);
    assert!(far.len() == 1 && far[0].starts_with(SIM), "{far:?}");
    assert_eq!(
        within(1.5),
        [
            "The confirmation sample is too small. Stratum fiction has 11 of the 38 clusters the power gate needs."
        ]
    );
}

#[test]
fn each_fault_row_yields_exactly_its_codes_and_paths() {
    let dir = study_dir();
    for row in faults() {
        let d = named(&edited(&row.edits), dir.path());
        assert_eq!(codes_paths(&d), row.want, "{:?}", row.edits);
    }
}

#[test]
fn the_named_rules_are_the_expected_set_and_rows_cover_the_pure_ones() {
    let in_file: BTreeSet<_> = v0()
        .iter()
        .filter(|r| matches!(r.check, rules::Check::Named(_)))
        .map(|r| r.code.clone())
        .collect();
    let want: BTreeSet<String> = [
        "E-LBL-001",
        "E-REF-002",
        "W-REF-001",
        "E-REF-003",
        "E-CNF-001",
        "E-CNF-002",
        "E-CNF-003",
        "E-CNF-004",
        "E-CNF-005",
        "E-CNF-006",
        "E-REF-001",
        "E-BUD-001",
        "E-END-001",
        "E-END-002",
        "E-MUT-001",
        "E-MUT-002",
        "E-MUT-003",
        "E-ISO-001",
        "E-PTH-001",
        "E-HSH-001",
        "E-SEC-001",
        "E-UNT-001",
        "E-UNT-002",
        "E-SEL-001",
        "E-TYP-001",
        "E-HYP-001",
        "E-HYP-002",
        "E-HYP-003",
        "E-HYP-004",
        "E-STR-001",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(in_file, want);
    // E-CNF-005 and E-HSH-001 read files and E-STR-001 the stream; their tests are below.
    // E-SEC-001 is in tests/scan.rs.
    let in_rows: BTreeSet<String> = faults()
        .iter()
        .flat_map(|r| r.want.iter().map(|w| w.0.to_string()))
        .collect();
    let missing: Vec<_> = want
        .difference(&in_rows)
        .filter(|c| {
            !matches!(
                c.as_str(),
                "E-CNF-005" | "E-HSH-001" | "E-SEC-001" | "E-STR-001"
            )
        })
        .collect();
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn boundaries_hold_on_both_sides() {
    let dir = study_dir();
    let passes = |edits: Vec<Edit>| {
        let d = named(&edited(&edits), dir.path());
        assert!(d.is_empty(), "{edits:?}: {d:?}");
    };
    // 22 clusters pass where 21 fail (table), with 1 unit each as with 5.
    passes(power_edits(5, 22));
    passes(power_edits(1, 22));
    passes(vec![("/mutations/depth_cap", j(json!(2)))]);
    passes(vec![("/mutations/depth_cap", j(json!(0)))]);
    passes(vec![("/scope/min_isolation", j(json!(2)))]);
    passes(vec![("/scope/min_isolation", j(json!(0)))]);
    passes(vec![
        ("/mutations/seeds/spawn", j(json!("a/"))),
        ("/mutations/seeds/report", j(json!("b/"))),
    ]);
    for key in ["cpu_hours", "wall_hours", "money_usd"] {
        let mut s = edited(&[
            ("/scope/budget/cpu_hours", j(json!(0.0))),
            ("/scope/budget/wall_hours", j(json!(0.0))),
            ("/scope/budget/money_usd", j(json!(0.0))),
        ]);
        s["scope"]["budget"][key] = json!(0.5);
        assert!(named(&s, dir.path()).is_empty(), "{key}");
    }
    // A `..` component is unsafe; a name that contains two dots is not.
    passes(vec![("/inputs/0/path", j(json!("data/a..b.txt")))]);
    // `a//b` is the same as `a/b`.
    passes(vec![("/inputs/0/path", j(json!("data//x.txt")))]);
    for when in ["H1.pass", "H1.fail"] {
        passes(route(one(when, "f.toml")));
    }
    // The planted-spawn template: depth 2, declared trigger, exploratory.
    let p = spec_at("../../templates/planted-spawn.toml");
    assert!(named(&p, dir.path()).is_empty());
}

#[test]
fn power_gate_problem_states_both_numbers() {
    let dir = study_dir();
    let d = named(&edited(&and(near(), power_edits(5, 9))), dir.path());
    assert_eq!(codes_paths(&d), [POWER]);
    assert_eq!(
        d[0].problem,
        "The confirmation sample is too small. Stratum fiction has 9 of the 10 clusters the power gate needs."
    );
    let pooled = named(
        &edited(&and(
            near(),
            vec![hyp("strata", j(json!([]))), ("/inputs", j(inputs(9, 5)))],
        )),
        dir.path(),
    );
    assert_eq!(
        pooled[0].problem,
        "The confirmation sample is too small. The pooled group has 9 of the 10 clusters the power gate needs."
    );
}

/// `ok()`'s context with a box whose stream of `log` gives `next`.
fn streamed(dir: &Path, log: &'static str, next: Result<Next, String>) -> CheckCtx {
    CheckCtx {
        records: Box::new(common::Streamed {
            studies: holding(pilot_estimates()),
            log,
            next,
        }),
        ..ctx(dir)
    }
}

/// The z floor's details only: a floor that passes gives way to the simulation.
fn floor_details(d: Vec<Diagnostic>) -> Vec<String> {
    d.into_iter()
        .map(|d| d.problem.rsplit_once(". ").unwrap().1.to_string())
        .filter(|d| d.ends_with(" clusters the power gate needs."))
        .collect()
}

#[test]
fn the_z_floor_sizes_at_the_distance_from_power_at_to_the_margin() {
    // `near()` moved up by 3: margin 3, power_at 2, range [-2, 8]. The distance is still 1,
    // so 5 clusters are short of the same 10; the sum 5 would ask for 1.
    let dir = study_dir();
    let mut e = and(near(), power_edits(5, 5));
    e.extend([
        hyp("claim", j(json!({ "below": 3.0 }))),
        hyp("power_at", j(json!(2.0))),
        hyp("statistic", j(json!({ "mean": [-2.0, 8.0] }))),
    ]);
    assert_eq!(
        floor_details(named(&edited(&e), dir.path())),
        ["Stratum fiction has 5 of the 10 clusters the power gate needs."]
    );
}

#[test]
fn power_gate_uses_the_stream_level_power_share_and_defaults() {
    let dir = study_dir();
    let detail = |edits: Vec<Edit>| floor_details(named(&edited(&edits), dir.path()));
    // The stream's level 0.05 (nine discoveries closed) needs 5 clusters.
    let ninth = Ok(Next {
        slot: 10,
        level: 0.05,
    });
    let at_ninth = |edits: Vec<Edit>| {
        let ctx = streamed(dir.path(), "planlog", ninth.clone());
        floor_details(rules::check_named(&edited(&edits), v0(), &ctx))
    };
    let mut e = and(near(), power_edits(5, 5));
    assert!(at_ninth(e.clone()).is_empty());
    // The first slot's level, 0.005, needs 10.
    assert_eq!(
        detail(e.clone()),
        ["Stratum fiction has 5 of the 10 clusters the power gate needs."]
    );
    e[3] = ("/inputs", j(inputs(4, 5)));
    assert_eq!(
        at_ninth(e),
        ["Stratum fiction has 4 of the 5 clusters the power gate needs."]
    );
    // Higher power needs more clusters.
    let mut e = and(near(), power_edits(5, 11));
    assert!(detail(e.clone()).is_empty());
    e.push(("/design/power", j(json!(0.9))));
    assert_eq!(
        detail(e),
        ["Stratum fiction has 11 of the 12 clusters the power gate needs."]
    );
    // A share of 0.5 halves the level: 11 clusters, each hypothesis.
    let mut halves = shared(&[0.5, 0.5]);
    for i in 0..2 {
        for (field, v) in [
            ("power_at", json!(-1.0)),
            ("statistic", json!({ "mean": [-5.0, 5.0] })),
        ] {
            let ptr: &'static str =
                Box::leak(format!("/question/hypotheses/{i}/{field}").into_boxed_str());
            halves.push((ptr, j(v)));
        }
    }
    halves.extend(power_edits(5, 11));
    assert!(detail(halves.clone()).is_empty());
    halves.extend(power_edits(5, 10));
    assert_eq!(
        detail(halves),
        [
            "Stratum fiction has 10 of the 11 clusters the power gate needs.",
            "Stratum fiction has 10 of the 11 clusters the power gate needs."
        ]
    );
    // An icc of 0.1 raises the need to 14.
    let icc = holding(vec![
        common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(0.1)),
        common::estimate(&[("arm-b", 1.0, json!({ "bits": 4 }))], "", 4.0, Some(0.1)),
    ]);
    let spec = edited(&and(near(), power_edits(5, 11)));
    let d = rules::check_named(&spec, v0(), &with_box(dir.path(), icc));
    assert!(
        d[0].problem
            .ends_with("Stratum fiction has 11 of the 14 clusters the power gate needs."),
        "{}",
        d[0].problem
    );
}

#[test]
fn a_confirmatory_plan_needs_a_level_from_the_stream_of_its_log() {
    let dir = study_dir();
    let run = |s: &Value, log, next: Result<Next, String>| {
        rules::check_named(s, v0(), &streamed(dir.path(), log, next))
    };
    const NO_LEVEL: &str = "The stream gives this confirmatory study no level.";
    // Slot 11 has level 0. E-STR-001 alone: the power gate has no level to size at.
    let beyond = || {
        Ok(Next {
            slot: 11,
            level: 0.0,
        })
    };
    let d = run(&ok(), "planlog", beyond());
    assert_eq!(codes_paths(&d), [("E-STR-001", "study.mode")]);
    assert_eq!(
        d[0].problem,
        format!("{NO_LEVEL} Slot 11 is beyond the 10 slots of the planlog stream.")
    );
    // A stream log that cannot be read fails closed.
    let d = run(&ok(), "planlog", Err("line 1 is not a whole record".into()));
    assert_eq!(codes_paths(&d), [("E-STR-001", "study.mode")]);
    assert_eq!(
        d[0].problem,
        format!("{NO_LEVEL} The planlog stream cannot be read: line 1 is not a whole record.")
    );
    // A test study takes its level from the test stream.
    let testing = edited(&[("/study/test_log", j(json!(true)))]);
    let d = run(&testing, "planlog", beyond());
    assert_eq!(
        d[0].problem,
        format!("{NO_LEVEL} The testlog stream cannot be read: no testlog stream here.")
    );
    let first = || {
        Ok(Next {
            slot: 1,
            level: 0.005,
        })
    };
    assert!(run(&testing, "testlog", first()).is_empty());
    assert!(run(&ok(), "planlog", first()).is_empty());
    // An exploratory plan takes no level; a test_log that is not a bool is the generic
    // rules', and the gate skips it.
    let exploratory = edited(&exploring(vec![]));
    assert!(run(&exploratory, "planlog", Err("unread".into())).is_empty());
    let mistyped = edited(&[("/study/test_log", j(json!("yes")))]);
    assert!(run(&mistyped, "planlog", Err("unread".into())).is_empty());
}

/// `ok()`'s box, whose first stream read fails, as a read torn by a concurrent write would,
/// and whose later reads give the first slot. It counts its stream reads.
struct Torn {
    studies: common::Studies,
    reads: Rc<std::cell::Cell<u32>>,
}

impl BoxRecords for Torn {
    fn study(&self, id: &str) -> Result<Option<Vec<records::Record>>, String> {
        self.studies.study(id)
    }

    fn stream_next(&self, _: &str) -> Result<Next, String> {
        self.reads.set(self.reads.get() + 1);
        if self.reads.get() == 1 {
            Err("line 2 is not a whole record".into())
        } else {
            Ok(Next {
                slot: 1,
                level: 0.005,
            })
        }
    }

    fn stream_closed(&self, _: &str, _: &str, _: u64) -> Result<bool, String> {
        Ok(false)
    }
}

/// A box that counts the reads of a study's records.
struct Counted {
    studies: common::Studies,
    reads: Rc<std::cell::Cell<u32>>,
}

impl BoxRecords for Counted {
    fn study(&self, id: &str) -> Result<Option<Vec<records::Record>>, String> {
        self.reads.set(self.reads.get() + 1);
        self.studies.study(id)
    }

    fn stream_next(&self, log: &str) -> Result<Next, String> {
        self.studies.stream_next(log)
    }

    fn stream_closed(&self, log: &str, study: &str, slot: u64) -> Result<bool, String> {
        self.studies.stream_closed(log, study, slot)
    }
}

#[test]
fn the_power_report_shows_the_simulation_the_rules_ran() {
    let dir = study_dir();
    let reads = Rc::new(std::cell::Cell::new(0));
    let counted = CheckCtx {
        records: Box::new(Counted {
            studies: holding(pilot_estimates()),
            reads: reads.clone(),
        }),
        ..ctx(dir.path())
    };
    assert!(rules::check_named(&ok(), v0(), &counted).is_empty());
    let read = reads.get();
    let power = rules::power(&ok(), &counted);
    assert_eq!(power.len(), 1);
    // A second simulation would read the pilot's estimates again.
    assert_eq!(reads.get(), read);
    // Another plan in the same check context is simulated on its own.
    let other = edited(&near());
    let fresh = rules::power(&other, &ctx(dir.path()));
    assert_ne!(fresh, power);
    assert_eq!(rules::power(&other, &counted), fresh);
    assert!(reads.get() > read);
}

#[test]
fn every_rule_of_one_check_sees_one_stream_read() {
    let dir = study_dir();
    let reads = Rc::new(std::cell::Cell::new(0));
    let torn = Torn {
        studies: holding(pilot_estimates()),
        reads: reads.clone(),
    };
    let ctx = CheckCtx {
        records: Box::new(rules::ReadOnce::new(torn)),
        ..ctx(dir.path())
    };
    // The gate reads first. Had a later rule read again, it would see a level and pass a plan
    // the gate never sized.
    let d = rules::check_named(&ok(), v0(), &ctx);
    assert_eq!(codes_paths(&d), [("E-STR-001", "study.mode")]);
    assert!(
        d[0].problem
            .ends_with("The planlog stream cannot be read: line 2 is not a whole record."),
        "{}",
        d[0].problem
    );
    // What check shows and register spends is that read too: an error, never no level.
    assert_eq!(
        rules::granted(&ok(), &ctx),
        Err("line 2 is not a whole record".to_string())
    );
    assert!(rules::power(&ok(), &ctx).is_empty());
    assert_eq!(reads.get(), 1);
    // An exploratory plan takes no level and reads nothing.
    let exploratory = edited(&exploring(vec![]));
    assert_eq!(rules::granted(&exploratory, &ctx), Ok(None));
    assert_eq!(reads.get(), 1);
}

/// A box that answers every closed-slot question with `closed`, for `(planlog, pilot, 3)`.
struct Slots(Result<bool, String>);

impl BoxRecords for Slots {
    fn study(&self, _: &str) -> Result<Option<Vec<records::Record>>, String> {
        Ok(None)
    }

    fn stream_next(&self, _: &str) -> Result<Next, String> {
        Err("not read".into())
    }

    fn stream_closed(&self, log: &str, study: &str, slot: u64) -> Result<bool, String> {
        assert_eq!((log, study, slot), ("planlog", "pilot", 3));
        self.0.clone()
    }
}

#[test]
fn read_once_passes_closed_slots_through() {
    for closed in [Ok(false), Ok(true), Err("bad line".to_string())] {
        let once = rules::ReadOnce::new(Slots(closed.clone()));
        assert_eq!(once.stream_closed("planlog", "pilot", 3), closed);
    }
}

#[test]
fn confirmatory_rules_do_not_fire_in_exploratory_mode() {
    let dir = study_dir();
    let s = edited(&[
        ("/study/mode", j(json!("exploratory"))),
        UNDECLARED,
        hyp("claim", None),
        hyp("alpha_share", j(json!(0.0))),
        hyp("strata", j(json!([]))),
        ("/inputs", j(inputs(0, 5))),
    ]);
    assert!(named(&s, dir.path()).is_empty());
    // A claim with its power_at on the wrong side, too few clusters, and a reference to this
    // study or to one that has not run: none of it is read in an exploratory study.
    for estimates in ["my-study", "later"] {
        let s = edited(&exploring(vec![
            hyp("power_at", j(json!(1.0))),
            hyp("estimates", j(json!({ "from": estimates }))),
            ("/inputs", j(inputs(2, 5))),
        ]));
        assert!(named(&s, dir.path()).is_empty(), "{estimates}");
    }
    let s = edited(&exploring(vec![("/inputs", j(inputs(2, 5)))]));
    assert!(named(&s, dir.path()).is_empty());
    assert!(named(&edited(&exploring(shared(&[0.5, 0.4]))), dir.path()).is_empty());
    // And E-LBL-001 does not fire in confirmatory mode.
    let s = edited(&[("/study/label", j(json!("confirmed")))]);
    assert!(named(&s, dir.path()).is_empty());
}

const WHEN: (&str, &str) = ("E-END-001", "scope.end_of_run.follow_on[0].when");

#[test]
fn done_routes_whatever_the_result() {
    let dir = study_dir();
    // `done` needs no claim: it holds in a confirmatory plan and beside a descriptive one.
    for edits in [
        route(one("done", "f.toml")),
        exploring(route(one("done", "f.toml"))),
        exploring(and(
            vec![("/question/hypotheses", j(json!([descriptive("P1")])))],
            route(one("done", "f.toml")),
        )),
    ] {
        assert!(named(&edited(&edits), dir.path()).is_empty(), "{edits:?}");
    }
    // Only the bare word: `done` is no hypothesis id and takes no suffix.
    for when in ["Done", "done.pass", "H1.done", "done "] {
        let d = named(&edited(&route(one(when, "f.toml"))), dir.path());
        assert_eq!(codes_paths(&d), [WHEN], "{when}");
    }
    let d = named(&edited(&route(one("H1.pass", "f.toml"))), dir.path());
    assert!(d.is_empty());
    assert_eq!(
        v0().iter()
            .find(|r| r.code == "E-END-001")
            .unwrap()
            .instruction,
        "For route follow-on, list at least one follow_on entry. Write each when as done, which runs the follow-on whatever the result, or as <hypothesis id>.pass or <hypothesis id>.fail, using a hypothesis that has a claim."
    );
}

#[test]
fn an_exploratory_claim_may_route_on_pass() {
    let dir = study_dir();
    for when in ["H1.pass", "H1.fail"] {
        let d = named(&edited(&exploring(route(one(when, "f.toml")))), dir.path());
        assert!(d.is_empty(), "{when}: {d:?}");
    }
    // A descriptive hypothesis has no pass or fail.
    for when in ["P1.pass", "P1.fail"] {
        let d = named(&edited(&exploring(route(one(when, "f.toml")))), dir.path());
        assert_eq!(codes_paths(&d), [WHEN], "{when}");
    }
}

#[test]
fn alpha_shares_that_miss_one_state_their_sum() {
    let dir = study_dir();
    let d = named(&edited(&shared(&[0.5, 0.4])), dir.path());
    assert_eq!(
        d[0].problem,
        "A confirmatory hypothesis has no alpha share above 0 and at most 1, or the shares do not sum to 1. The shares sum to 0.9."
    );
    let d = named(&edited(&[hyp("alpha_share", j(json!(0.0)))]), dir.path());
    assert_eq!(
        d[0].problem,
        "A confirmatory hypothesis has no alpha share above 0 and at most 1, or the shares do not sum to 1."
    );
}

#[test]
fn the_template_claim_example_passes_once_uncommented() {
    let dir = study_dir();
    for t in ["numeric-ablation", "planted-spawn"] {
        let text = std::fs::read_to_string(format!("../../templates/{t}.toml")).unwrap();
        let v = ignorarium::spec::parse(common::with_claim(text).as_bytes())
            .unwrap()
            .value;
        let h = &v["question"]["hypotheses"][0];
        assert!(h["claim"].is_object() && h["statistic"].is_object(), "{t}");
        assert!(h["alpha_share"].is_number(), "{t}");
        let (_, _, d) = rules::check_all(&v, v0(), &ctx(dir.path()));
        let errors: Vec<_> = d.iter().filter(|x| x.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "{t}: {errors:?}");
    }
}

fn junk() -> Vec<Value> {
    vec![
        json!("text"),
        json!(7),
        json!(i64::MAX),
        json!(-1.5),
        json!([]),
        json!([1, "a"]),
        json!({}),
        json!(true),
    ]
}

fn nodes(v: &Value, ptr: &str, out: &mut Vec<String>) {
    let children: Vec<(String, &Value)> = match v {
        Value::Object(m) => m.iter().map(|(k, c)| (format!("{ptr}/{k}"), c)).collect(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, c)| (format!("{ptr}/{i}"), c))
            .collect(),
        _ => vec![],
    };
    for (p, c) in children {
        out.push(p.clone());
        nodes(c, &p, out);
    }
}

#[test]
fn named_checks_never_panic_on_missing_or_mistyped_fields() {
    let dir = study_dir();
    assert!(named(&json!({}), dir.path()).is_empty());
    let template = spec_at("../../templates/numeric-ablation.toml");
    let strings: serde_json::Map<String, Value> = template
        .as_object()
        .unwrap()
        .keys()
        .map(|k| (k.clone(), json!("x")))
        .collect();
    named(&Value::Object(strings), dir.path());
    // A rich confirmatory plan with every node replaced by each junk value, and removed. Its 9
    // clusters fail the z floor, so few variants reach the slower simulation.
    let mut base = edited(&and(
        route(one("H1.pass", "f.toml")),
        and(near(), power_edits(5, 9)),
    ));
    set(&mut base, "/inputs/0/role", j(json!("confirmation")));
    set(&mut base, "/inputs/0/path", j(json!("f.toml")));
    let mut ptrs = vec![];
    nodes(&base, "", &mut ptrs);
    assert!(ptrs.len() > 100);
    let mut ran = 0;
    for ptr in &ptrs {
        for v in junk() {
            let mut s = base.clone();
            set(&mut s, ptr, Some(v));
            named(&s, dir.path());
            ran += 1;
        }
        let parent = ptr.rsplit_once('/').unwrap().0;
        if base.pointer(parent).is_some_and(Value::is_object) {
            let mut s = base.clone();
            set(&mut s, ptr, None);
            named(&s, dir.path());
        }
    }
    assert!(ran > 600);
}

// ---- checks that read files ----

const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// `ok()` with its first input at `path`, pinned by `sha`, in `role`, with units and a stratum.
fn with_input(path: &str, sha: &str, role: &str) -> Value {
    let mut s = ok();
    set(&mut s, "/inputs/0/path", j(json!(path)));
    set(&mut s, "/inputs/0/sha256", j(json!(sha)));
    set(&mut s, "/inputs/0/role", j(json!(role)));
    set(&mut s, "/inputs/0/units", j(json!(5)));
    set(&mut s, "/inputs/0/stratum", j(json!("fiction")));
    s
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[test]
fn input_hash_must_match_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/a.txt", b"abc");
    let run = |path: &str, sha: &str| named(&with_input(path, sha, "calibration"), dir.path());
    assert!(run("data/a.txt", ABC).is_empty());
    let hsh = [("E-HSH-001", "inputs[0].sha256")];
    assert_eq!(codes_paths(&run("data/a.txt", &"0".repeat(64))), hsh);
    // A missing file with a hash fires. Without a hash only the generic warning applies.
    assert_eq!(codes_paths(&run("data/none.txt", ABC)), hsh);
    assert!(run("data/none.txt", "").is_empty());
    // An empty hash on an existing file is not compared.
    assert!(run("data/a.txt", "").is_empty());
    // A folder is not a file.
    assert_eq!(codes_paths(&run("data", ABC)), hsh);
}

#[test]
fn missing_hash_is_a_warning_from_the_generic_rule() {
    let mut s = spec_at("../../templates/numeric-ablation.toml");
    let w = |s: &Value| -> Vec<Diagnostic> {
        rules::check_generic(s, v0())
            .into_iter()
            .filter(|x| x.code == "W-HSH-001")
            .collect()
    };
    let d = w(&s);
    assert_eq!(
        codes_paths(&d),
        [
            ("W-HSH-001", "inputs[0].sha256"),
            ("W-HSH-001", "inputs[1].sha256")
        ]
    );
    assert_eq!(d[0].severity, Severity::Warning);
    assert_eq!(
        d[0].instruction,
        "Pin the input: add its SHA-256, 64 lowercase hex characters."
    );
    set(&mut s, "/inputs/0/sha256", j(json!(ABC)));
    set(&mut s, "/inputs/1/sha256", j(json!(ABC)));
    assert!(w(&s).is_empty());
}

#[test]
fn follow_on_files_are_found_under_the_study_folder() {
    let dir = tempfile::tempdir().unwrap();
    let s = edited(&route(json!([
        { "when": "H1.pass", "spec": "follow/a.toml" },
        { "when": "H1.fail", "spec": "follow/b.toml" },
    ])));
    write(dir.path(), "follow/a.toml", b"x = 1\n");
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-END-002", "scope.end_of_run.follow_on[1].spec")]
    );
    write(dir.path(), "follow/b.toml", b"x = 1\n");
    assert!(named(&s, dir.path()).is_empty());
}

type Seen = Rc<RefCell<Vec<(String, String)>>>;

struct Fake {
    exact: Option<String>,
    normalized: Option<String>,
    seen: Seen,
}

impl PriorUse for Fake {
    fn used(&self, exact: &str, normalized: &str) -> Result<Option<String>, PriorError> {
        self.seen
            .borrow_mut()
            .push((exact.into(), normalized.into()));
        if self.exact.as_deref() == Some(exact) {
            return Ok(Some("entry-exact".into()));
        }
        if self.normalized.as_deref() == Some(normalized) {
            return Ok(Some("entry-norm".into()));
        }
        Ok(None)
    }
}

struct Broken;

impl PriorUse for Broken {
    fn used(&self, _: &str, _: &str) -> Result<Option<String>, PriorError> {
        Err(PriorError("leaf 3 is not a leaf".into()))
    }
}

#[test]
fn an_unreadable_plan_log_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"abc");
    let d = fresh(
        &with_input("d.txt", "", "confirmation"),
        dir.path(),
        Box::new(Broken),
    );
    assert_eq!(codes_paths(&d), [("E-CNF-005", "inputs[0].path")]);
    assert!(
        d[0].problem
            .ends_with("The plan log cannot be read: leaf 3 is not a leaf."),
        "{}",
        d[0].problem
    );
}

fn fake(exact: Option<&str>, normalized: Option<&str>) -> (Box<dyn PriorUse>, Seen) {
    let seen: Seen = Rc::default();
    let p = Fake {
        exact: exact.map(Into::into),
        normalized: normalized.map(Into::into),
        seen: seen.clone(),
    };
    (Box::new(p), seen)
}

fn fresh(spec: &Value, dir: &Path, prior: Box<dyn PriorUse>) -> Vec<Diagnostic> {
    rules::check_named(spec, v0(), &ctx_with(dir, prior))
}

#[test]
fn reused_selection_or_confirmation_data_is_refused_naming_the_entry() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"abc");
    for role in ["selection", "confirmation"] {
        let (prior, seen) = fake(Some(ABC), None);
        let d = fresh(&with_input("d.txt", "", role), dir.path(), prior);
        assert_eq!(codes_paths(&d), [("E-CNF-005", "inputs[0].path")], "{role}");
        assert!(
            d[0].problem.ends_with("The earlier entry is entry-exact."),
            "{}",
            d[0].problem
        );
        assert_eq!(seen.borrow().len(), 1);
    }
    // Other roles are not asked.
    for role in ["calibration", "model"] {
        let (prior, seen) = fake(Some(ABC), None);
        assert!(fresh(&with_input("d.txt", "", role), dir.path(), prior).is_empty());
        assert!(seen.borrow().is_empty(), "{role}");
    }
    // Unknown data passes.
    let (prior, _) = fake(Some("nope"), Some("nope"));
    assert!(fresh(&with_input("d.txt", "", "confirmation"), dir.path(), prior).is_empty());
}

#[test]
fn reuse_is_found_through_the_normalized_hash_too() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"Alpha \r\n  BETA\n");
    let (prior, seen) = fake(None, Some(&normalized_sha256("alpha beta")));
    let d = fresh(&with_input("d.txt", "", "confirmation"), dir.path(), prior);
    assert!(
        d[0].problem.ends_with("The earlier entry is entry-norm."),
        "{}",
        d[0].problem
    );
    assert_eq!(seen.borrow()[0].0, sha256_hex(b"Alpha \r\n  BETA\n"));
}

#[test]
fn non_utf8_data_passes_an_empty_normalized_hash() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = [0xff_u8, 0xfe, 0x00];
    write(dir.path(), "d.bin", &bytes);
    let (prior, seen) = fake(None, None);
    assert!(fresh(&with_input("d.bin", "", "selection"), dir.path(), prior).is_empty());
    assert_eq!(*seen.borrow(), [(sha256_hex(&bytes), String::new())]);
}

#[test]
fn freshness_skips_missing_unsafe_and_exploratory_inputs() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "d.txt", b"abc");
    let known = normalized_sha256("abc");
    let (prior, seen) = fake(Some(ABC), Some(&known));
    assert!(
        fresh(
            &with_input("gone.txt", "", "confirmation"),
            dir.path(),
            prior
        )
        .is_empty()
    );
    assert!(seen.borrow().is_empty());
    let (prior, seen) = fake(Some(ABC), Some(&known));
    let d = fresh(
        &with_input("../d.txt", "", "confirmation"),
        dir.path(),
        prior,
    );
    assert_eq!(codes_paths(&d), [("E-PTH-001", "inputs[0].path")]);
    assert!(seen.borrow().is_empty());
    let (prior, seen) = fake(Some(ABC), None);
    let mut s = with_input("d.txt", "", "confirmation");
    set(&mut s, "/study/mode", j(json!("exploratory")));
    set(&mut s, "/pins/deviations", None);
    assert!(fresh(&s, dir.path(), prior).is_empty());
    assert!(seen.borrow().is_empty());
}

#[cfg(unix)]
#[test]
fn symlinks_under_the_study_folder_are_refused_and_never_read() {
    use std::os::unix::fs::symlink;
    let outer = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write(outer.path(), "secret.txt", b"abc");
    write(dir.path(), "real/x.txt", b"abc");
    symlink(outer.path(), dir.path().join("linkdir")).unwrap();
    symlink(
        outer.path().join("secret.txt"),
        dir.path().join("linkfile.txt"),
    )
    .unwrap();
    symlink(dir.path().join("real"), dir.path().join("inner")).unwrap();
    for path in ["linkdir/secret.txt", "linkfile.txt", "inner/x.txt"] {
        let (prior, seen) = fake(Some(ABC), None);
        let d = fresh(&with_input(path, ABC, "confirmation"), dir.path(), prior);
        assert_eq!(codes_paths(&d), [("E-PTH-001", "inputs[0].path")], "{path}");
        assert!(seen.borrow().is_empty(), "{path}");
    }
    let s = edited(&route(one("H1.pass", "linkfile.txt")));
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-PTH-001", "scope.end_of_run.follow_on[0].spec")]
    );
    // A linked folder on the way to a follow-on spec.
    let s = edited(&route(one("H1.pass", "inner/x.txt")));
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-PTH-001", "scope.end_of_run.follow_on[0].spec")]
    );
    // A real path passes.
    assert!(named(&with_input("real/x.txt", ABC, "calibration"), dir.path()).is_empty());
}

#[test]
fn an_absent_power_says_it_is_missing() {
    let dir = study_dir();
    let d = named(&edited(&[("/design/power", None)]), dir.path());
    assert_eq!(codes_paths(&d), [POWER]);
    assert_eq!(
        d[0].problem,
        "The confirmation sample is too small. Cannot compute power: design.power is absent."
    );
    // A mistyped power skips the gate.
    assert!(named(&edited(&[("/design/power", j(json!("x")))]), dir.path()).is_empty());
    // Only a hypothesis the gate sizes is reported.
    let d = named(
        &edited(&[
            ("/design/power", None),
            ("/question/hypotheses", j(json!([descriptive("P1"), h1()]))),
        ]),
        dir.path(),
    );
    assert_eq!(
        codes_paths(&d),
        [
            ("E-CNF-002", "question.hypotheses[1]"),
            ("E-CNF-006", "question.hypotheses[0].claim")
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_pipe_is_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let ok = std::process::Command::new("mkfifo")
        .arg(dir.path().join("pipe"))
        .status()
        .unwrap()
        .success();
    assert!(ok);
    // Run in a thread: a check that opens the pipe blocks forever.
    let (tx, rx) = std::sync::mpsc::channel();
    let (spec, path) = (
        with_input("pipe", ABC, "confirmation"),
        dir.path().to_path_buf(),
    );
    std::thread::spawn(move || {
        let d = named(&spec, &path);
        let got: Vec<_> = d.iter().map(|x| (x.code.clone(), x.path.clone())).collect();
        let _ = tx.send(got);
    });
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the check blocked on a pipe");
    assert_eq!(
        got,
        [("E-HSH-001".to_string(), "inputs[0].sha256".to_string())]
    );
}

// ---- rule loading and gating ----

fn rule_file(extra: &str, check: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("r.toml");
    let text = format!(
        "[[rule]]\ncode = \"X-TST-001\"\nseverity = \"warning\"\nproblem = \"p\"\ninstruction = \"i\"\nwhy = \"w\"\nsources = []\n{extra}\n\
         [[rule.target]]\npath = \"mutations.depth_cap\"\ncheck = \"{check}\"\n"
    );
    std::fs::write(&file, text).unwrap();
    (dir, file)
}

#[test]
fn an_unknown_named_check_is_a_load_error_naming_the_code() {
    let (_dir, file) = rule_file("", "named:no-such-check");
    let e = rules::load(&file).unwrap_err().to_string();
    assert!(
        e.contains("X-TST-001") && e.contains("mutations.depth_cap") && e.contains("no-such-check"),
        "{e}"
    );
}

#[test]
fn a_named_rule_gates_on_when_like_a_generic_one() {
    let (dir, file) = rule_file("when = { mode = \"confirmatory\" }", "named:depth-cap");
    let r = rules::load(&file).unwrap();
    let mut s = edited(&[("/mutations/depth_cap", j(json!(9)))]);
    let c = ctx(dir.path());
    let d = rules::check_named(&s, &r, &c);
    assert_eq!(codes_paths(&d), [("X-TST-001", "mutations.depth_cap")]);
    assert_eq!(d[0].severity, Severity::Warning);
    set(&mut s, "/study/mode", j(json!("exploratory")));
    assert!(rules::check_named(&s, &r, &c).is_empty());
}

#[test]
fn bal_brief_is_refused_with_four_codes() {
    let dir = Path::new("tests/fixtures/specs");
    let s = spec_at("tests/fixtures/specs/bal-brief.toml");
    let confirm = std::fs::read(dir.join("bal-confirm.txt")).unwrap();
    let (prior, _) = fake(Some(&sha256_hex(&confirm)), None);
    let pilot = common::finished(vec![common::estimate(
        &[("arm-a", 1.0, json!({}))],
        "",
        4.0,
        Some(0.0),
    )]);
    let c = CheckCtx {
        records: Box::new(common::Studies(vec![(
            "bal-pilot",
            Ok(common::records_for("bal-pilot", &pilot)),
        )])),
        ..ctx_with(dir, prior)
    };
    let d = rules::check_named(&s, v0(), &c);
    let codes: BTreeSet<_> = d.iter().map(|x| x.code.as_str()).collect();
    for want in ["E-REF-003", "E-CNF-002", "E-CNF-005", "E-CNF-006"] {
        assert!(codes.contains(want), "{want}: {codes:?}");
    }
    let power = d.iter().find(|x| x.code == "E-CNF-002").unwrap();
    assert!(
        power
            .problem
            .ends_with("The pooled group has 1 of the 10 clusters the power gate needs."),
        "{}",
        power.problem
    );
}

#[test]
fn power_gate_numbers_at_the_guard_edges() {
    let dir = study_dir();
    let detail = |edits: Vec<Edit>, b: common::Studies| {
        let d = rules::check_named(&edited(&edits), v0(), &with_box(dir.path(), b));
        assert_eq!(d.len(), 1, "{edits:?}");
        d[0].problem.rsplit_once(". ").unwrap().1.to_string()
    };
    let floor = |extra: Vec<Edit>| and(and(near(), power_edits(5, 11)), extra);
    // power 0.999 needs 26 clusters at sd 2 and 5 units per cluster.
    assert_eq!(
        detail(
            floor(vec![("/design/power", j(json!(0.999)))]),
            holding(pilot_estimates())
        ),
        "Stratum fiction has 11 of the 26 clusters the power gate needs."
    );
    // An icc of 1: a cluster is worth one unit, whatever its size.
    let one = |icc| {
        holding(vec![
            common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(icc)),
            common::estimate(&[("arm-b", 1.0, json!({ "bits": 4 }))], "", 4.0, Some(icc)),
        ])
    };
    assert_eq!(
        detail(floor(vec![]), one(1.0)),
        "Stratum fiction has 11 of the 47 clusters the power gate needs."
    );
    // The largest integer saturates instead of overflowing.
    let e = vec![("/design/units_per_cluster", j(json!(i64::MAX)))];
    let d = rules::check_named(&edited(&e), v0(), &with_box(dir.path(), one(0.0)));
    assert!(d.is_empty(), "{d:?}");
}

/// `n` clusters of 5 units in each of strata `code` and `article`, 0.64 below the margin, with
/// cluster means in [-4, 4]: a z-test needs 23 clusters per stratum.
fn two_strata(n: usize) -> Vec<Edit> {
    let mut list = inputs(0, 5).as_array().unwrap().clone();
    list[1]["stratum"] = json!("code");
    for (i, stratum) in ["code", "article"].iter().enumerate() {
        for k in 0..n {
            let mut c = input(&format!("texts/c{i}{k:02}.txt"), "confirmation", 5);
            c["stratum"] = json!(stratum);
            list.push(c);
        }
    }
    vec![
        ("/inputs", j(Value::Array(list))),
        hyp("strata", j(json!(["code", "article"]))),
        hyp("power_at", j(json!(-0.64))),
        hyp("statistic", j(json!({ "mean": [-4.0, 4.0] }))),
    ]
}

fn problems(spec: &Value, dir: &Path) -> Vec<String> {
    named(spec, dir).into_iter().map(|d| d.problem).collect()
}

#[test]
fn the_gate_refuses_what_the_z_formula_passes() {
    let dir = study_dir();
    // 22 per stratum fall short of the z floor's 23, and the gate states the floor's numbers.
    let short = |s: &str| {
        format!(
            "The confirmation sample is too small. Stratum {s} has 22 of the 23 clusters the power gate needs."
        )
    };
    assert_eq!(
        problems(&edited(&two_strata(22)), dir.path()),
        [short("code"), short("article")]
    );
    // 26 pass the floor. The e-value test the box runs needs far more: every bound in both
    // strata must reach 1/0.005.
    let spec = edited(&two_strata(26));
    assert_eq!(
        problems(&spec, dir.path()),
        [format!(
            "{SIM}0.00 with 26 clusters in code and 26 in article; about 96 per stratum reach 0.80."
        )]
    );
    assert_eq!(
        rules::power(&spec, &ctx(dir.path())),
        [Power {
            hypothesis: "H1".into(),
            power: 0.0,
            target: 0.8,
            clusters: vec![("code".into(), 26), ("article".into(), 26)],
            needed: Some(96),
        }]
    );
}

#[test]
fn the_z_floor_fails_without_simulating() {
    // 1 below the margin with 1 unit per cluster a z-test needs 47 clusters. With 46 the gate
    // states the floor's numbers alone. With 47 it passes the floor and simulates the test,
    // which needs about twice as many.
    let dir = study_dir();
    let at = |n| problems(&edited(&and(near(), power_edits(1, n))), dir.path());
    assert_eq!(
        at(46),
        [
            "The confirmation sample is too small. Stratum fiction has 46 of the 47 clusters the power gate needs."
        ]
    );
    assert_eq!(
        at(47),
        [format!(
            "{SIM}0.05 with 47 clusters in fiction; about 94 per stratum reach 0.80."
        )]
    );
}

#[test]
fn a_quantile_claim_is_simulated_without_a_floor() {
    let dir = study_dir();
    let median = |extra: Vec<Edit>| {
        let mut e = extra;
        e.push(hyp("statistic", j(json!({ "quantile": 0.5 }))));
        problems(&edited(&e), dir.path())
    };
    // 30 below the margin every unit lies below it, so each cluster scores 1 and the e-value
    // after n clusters is 1.5^(n - 1): 15 clusters reach 1/0.005, 14 do not.
    assert!(median(power_edits(5, 15)).is_empty());
    assert_eq!(
        median(power_edits(5, 14)),
        [format!(
            "{SIM}0.00 with 14 clusters in fiction; about 15 per stratum reach 0.80."
        )]
    );
    // Just below the margin about half the units lie on each side, and 1,024 clusters per
    // stratum do not reach the target.
    assert_eq!(
        median(and(
            power_edits(1, 2),
            vec![hyp("power_at", j(json!(-0.01)))]
        )),
        [format!(
            "{SIM}0.00 with 2 clusters in fiction; 1024 per stratum do not reach 0.80."
        )]
    );
    // Pooled, the count is of clusters.
    assert_eq!(
        median(and(power_edits(5, 14), vec![hyp("strata", j(json!([])))])),
        [format!(
            "{SIM}0.00 with 14 clusters in the pooled group; about 15 clusters reach 0.80."
        )]
    );
    // One cluster, and a target given to three decimals.
    let mut e = power_edits(5, 1);
    e.push(("/design/power", j(json!(0.999))));
    assert_eq!(
        median(e),
        [format!(
            "{SIM}0.00 with 1 cluster in fiction; about 15 per stratum reach 0.999."
        )]
    );
    // Each stratum the claim lists, named in order.
    let mut list = inputs(0, 5).as_array().unwrap().clone();
    for (i, stratum) in ["fiction", "code", "article"].iter().enumerate() {
        for k in 0..2 {
            let mut c = input(&format!("texts/c{i}{k}.txt"), "confirmation", 5);
            c["stratum"] = json!(stratum);
            list.push(c);
        }
    }
    assert_eq!(
        median(vec![
            ("/inputs", j(Value::Array(list))),
            hyp("strata", j(json!(["fiction", "code", "article"]))),
        ]),
        [format!(
            "{SIM}0.00 with 2 clusters in fiction, 2 in code and 2 in article; about 15 per stratum reach 0.80."
        )]
    );
    // The 0.9-quantile of the units at -3: their mean lies 2 sd z(0.9) = 2.56 lower, where
    // 99.7% of units fall below the margin, against the 90% the null allows.
    let tail = vec![
        hyp("statistic", j(json!({ "quantile": 0.9 }))),
        hyp("power_at", j(json!(-3.0))),
    ];
    assert_eq!(
        problems(&edited(&and(power_edits(1, 2), tail)), dir.path()),
        [format!(
            "{SIM}0.00 with 2 clusters in fiction; about 115 per stratum reach 0.80."
        )]
    );
    // 1 below the margin a mean would fail the z floor with 2 clusters; a quantile has no
    // floor, and the simulation states its own numbers.
    assert_eq!(
        median(and(near(), power_edits(5, 2))),
        [format!(
            "{SIM}0.00 with 2 clusters in fiction; about 42 per stratum reach 0.80."
        )]
    );
}

#[test]
fn a_quantile_claim_too_large_to_simulate_fails_closed() {
    // A quantile draws every unit: 15 clusters of 1,000,000 units over 4,000 replicates are
    // 6e10 values, above the simulation's budget. The row cap fires too.
    let dir = study_dir();
    let mut e = power_edits(1_000_000, 15);
    e.push(hyp("statistic", j(json!({ "quantile": 0.5 }))));
    let d = named(&edited(&e), dir.path());
    assert_eq!(
        codes_paths(&d),
        [POWER, ("E-UNT-002", "design.units_per_cluster")]
    );
    assert_eq!(
        d[0].problem,
        "The confirmation sample is too small. Cannot simulate power: it would draw more than 100 million values."
    );
}

#[test]
fn a_plan_the_simulation_refuses_fails_closed() {
    // An analysis of variance gives no icc above 1; a record that holds one stops the gate.
    let held = |icc| {
        holding(vec![
            common::estimate(&[("arm-a", 1.0, json!({}))], "", 4.0, Some(icc)),
            common::estimate(&[("arm-b", 1.0, json!({ "bits": 4 }))], "", 4.0, Some(icc)),
        ])
    };
    let dir = study_dir();
    let d = rules::check_named(&ok(), v0(), &with_box(dir.path(), held(1.5)));
    assert_eq!(
        d.iter()
            .map(|x| (x.code.as_str(), x.problem.as_str()))
            .collect::<Vec<_>>(),
        [(
            "E-CNF-002",
            "The confirmation sample is too small. Cannot simulate power: invalid parameter: icc."
        )]
    );
    assert!(rules::power(&ok(), &with_box(dir.path(), held(1.5))).is_empty());
    assert!(rules::check_named(&ok(), v0(), &with_box(dir.path(), held(1.0))).is_empty());
}

#[test]
fn a_power_that_meets_the_target_exactly_passes_and_searches_nothing() {
    // The seed comes from the check's spec hash, not from design.power, so the same plan
    // simulates the same power at any target: set the target to that power.
    let dir = study_dir();
    // `near()` with 60 clusters simulates a power of about 0.5.
    let mut spec = edited(&and(near(), power_edits(5, 60)));
    let p = rules::power(&spec, &ctx(dir.path()))[0].power;
    assert!((0.5..0.999).contains(&p), "{p}");
    spec["design"]["power"] = json!(p);
    let at = rules::power(&spec, &ctx(dir.path()));
    assert_eq!((at[0].power, at[0].target, at[0].needed), (p, p, None));
    let d = named(&spec, dir.path());
    assert!(d.iter().all(|x| x.code != "E-CNF-002"), "{d:?}");
}

#[test]
fn the_power_is_seeded_by_the_spec_hash_and_the_hypothesis_id() {
    // Two hypotheses alike but for their ids, each at half the level with 24 clusters, where
    // the simulated power lies between 0 and 1.
    let dir = study_dir();
    let mut e = shared(&[0.5, 0.5]);
    e.extend(power_edits(5, 24));
    let spec = edited(&e);
    let of = |hash: [u8; 32]| {
        rules::power(
            &spec,
            &CheckCtx {
                spec_hash: hash,
                ..ctx(dir.path())
            },
        )
        .iter()
        .map(|p| p.power)
        .collect::<Vec<_>>()
    };
    let base = of([0; 32]);
    assert_eq!(base.len(), 2);
    assert!(base.iter().all(|p| 0.1 < *p && *p < 0.9), "{base:?}");
    assert_ne!(base[0], base[1]);
    assert_eq!(of([0; 32]), base);
    let other = of([1; 32]);
    assert!(
        other[0] != base[0] && other[1] != base[1],
        "{base:?} {other:?}"
    );
    // An id that fails its format, or that the secret scan flags, shows as <name>.
    for id in [
        "Pasted Key".to_string(),
        format!("{}{}", "AKIA", "Q".repeat(16)),
    ] {
        let mut s = spec.clone();
        s["question"]["hypotheses"][0]["id"] = json!(id);
        assert_eq!(rules::power(&s, &ctx(dir.path()))[0].hypothesis, "<name>");
    }
    assert_eq!(rules::power(&spec, &ctx(dir.path()))[1].hypothesis, "H2");
    // No power is simulated in an exploratory study, or where the z floor fails.
    assert!(rules::power(&edited(&exploring(vec![])), &ctx(dir.path())).is_empty());
    let floor = edited(&and(near(), power_edits(5, 9)));
    assert!(rules::power(&floor, &ctx(dir.path())).is_empty());
}

/// `inputs[0].path` matches `inputs[*].path`, and `params.bits` matches `params.*`.
fn matches_pattern(path: &str, pattern: &str) -> bool {
    let (p, q): (Vec<_>, Vec<_>) = (path.split('.').collect(), pattern.split('.').collect());
    p.len() == q.len()
        && p.iter().zip(&q).all(|(a, b)| match b.strip_suffix("[*]") {
            Some(name) => a
                .strip_prefix(name)
                .is_some_and(|r| r.starts_with('[') && r.ends_with(']')),
            None => *b == "*" || a == b,
        })
}

#[test]
fn every_named_diagnostic_is_reported_at_a_path_its_target_matches() {
    let dir = study_dir();
    let mut seen = BTreeSet::new();
    for row in faults() {
        let d = named(&edited(&row.edits), dir.path());
        for x in &d {
            let ok = v0()
                .iter()
                .filter(|r| r.code == x.code)
                .any(|r| matches_pattern(&x.path, &r.path));
            assert!(ok, "{} at {}", x.code, x.path);
            seen.insert((x.code.clone(), x.path.clone()));
        }
    }
    assert!(seen.len() > 15, "{}", seen.len());
    assert!(matches_pattern("a[12].b", "a[*].b") && !matches_pattern("a.b", "a[*].b"));
    assert!(matches_pattern("a.k.b", "a.*.b") && !matches_pattern("a.k", "a.*.b"));
}

// ---- inputs from the store ----

/// Put `bytes` in the store under `dir` and return its digest and meta.
fn stored(dir: &Path, bytes: &[u8]) -> (String, ignorarium::store::Meta) {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("f"), bytes).unwrap();
    ignorarium::store::Store::open(dir)
        .unwrap()
        .import(&src.path().join("f"))
        .unwrap()
}

#[test]
fn a_stored_input_passes_without_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, meta) = stored(dir.path(), b"Some  Text");
    let spec = with_input("data/a.txt", &digest, "calibration");
    assert!(named(&spec, dir.path()).is_empty());
    // The store comes first: a different file at the path does not matter.
    write(dir.path(), "data/a.txt", b"other bytes");
    assert!(named(&spec, dir.path()).is_empty());
    let store = ignorarium::store::Store::open(dir.path()).unwrap();
    let got = ignorarium::cli::check::input_hashes(&store, dir.path(), &spec);
    assert_eq!(
        got[0],
        (
            "data/a.txt".to_string(),
            Some((digest, meta.normalized_sha256))
        )
    );
    assert_eq!(got[0].1.as_ref().unwrap().1, normalized_sha256("some text"));
}

#[test]
fn a_stored_input_counts_for_freshness_through_its_meta() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, meta) = stored(dir.path(), b"Some  Text");
    let (prior, seen) = fake(None, Some(&meta.normalized_sha256));
    let d = fresh(
        &with_input("data/a.txt", &digest, "confirmation"),
        dir.path(),
        prior,
    );
    assert_eq!(codes_paths(&d), [("E-CNF-005", "inputs[0].path")]);
    assert!(d[0].problem.ends_with("The earlier entry is entry-norm."));
    assert_eq!(*seen.borrow(), [(digest, meta.normalized_sha256)]);
}

#[test]
fn an_input_in_neither_place_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, _) = stored(dir.path(), b"abc!");
    let run = |path: &str, sha: &str| named(&with_input(path, sha, "calibration"), dir.path());
    let hsh = [("E-HSH-001", "inputs[0].sha256")];
    // The pin names no stored object and no file is there.
    assert_eq!(codes_paths(&run("data/none.txt", ABC)), hsh);
    // The pin is a stored digest: it passes.
    assert!(run("data/none.txt", &digest).is_empty());
    // A pin that is no digest at all, with a file under the path whose hash differs.
    write(dir.path(), "data/a.txt", b"abc");
    assert_eq!(codes_paths(&run("data/a.txt", "xyz")), hsh);
    assert!(run("data/a.txt", ABC).is_empty());
}

#[test]
fn an_input_inside_another_inputs_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let with_second = |first: &str, second: &str| {
        let mut s = with_input(first, "", "calibration");
        let mut extra = s["inputs"][0].clone();
        extra["path"] = json!(second);
        s["inputs"].as_array_mut().unwrap().insert(1, extra);
        named(&s, dir.path())
    };
    let nested = [("E-PTH-001", "inputs[1].path")];
    assert_eq!(codes_paths(&with_second("data/a", "data/a/b")), nested);
    // The inner path is flagged whichever comes first.
    assert_eq!(
        codes_paths(&with_second("data/a/b", "data/a")),
        [("E-PTH-001", "inputs[0].path")]
    );
    // An empty path is unsafe, and holds no other input.
    assert_eq!(
        codes_paths(&with_second("", "data/a")),
        [("E-PTH-001", "inputs[0].path")]
    );
    // A shared name prefix is not a shared folder.
    assert!(with_second("data/a", "data/ab").is_empty());
    assert!(with_second("data/a", "data/b/a").is_empty());
    // Another spelling of the same path names the same file: the later input is flagged.
    for other in ["data//a", "data/a/", "data//a//"] {
        assert_eq!(
            codes_paths(&with_second("data/a", other)),
            nested,
            "{other}"
        );
        assert_eq!(
            codes_paths(&with_second(other, "data/a")),
            nested,
            "{other}"
        );
    }
}

#[test]
fn a_stored_input_whose_blob_is_gone_or_changed_is_not_held() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, _) = stored(dir.path(), b"abcdef");
    let blob = dir.path().join("store").join(&digest);
    let spec = with_input("data/none.txt", &digest, "calibration");
    assert!(named(&spec, dir.path()).is_empty());
    let store = ignorarium::store::Store::open(dir.path()).unwrap();
    let hsh = [("E-HSH-001", "inputs[0].sha256")];
    std::fs::remove_file(&blob).unwrap();
    assert_eq!(codes_paths(&named(&spec, dir.path())), hsh);
    // Another size: the check can afford a stat, not a read.
    std::fs::write(&blob, b"abc").unwrap();
    assert_eq!(codes_paths(&named(&spec, dir.path())), hsh);
    assert_eq!(
        ignorarium::cli::check::input_hashes(&store, dir.path(), &spec)[0].1,
        None
    );
    std::fs::remove_file(&blob).unwrap();
    std::fs::create_dir(&blob).unwrap();
    assert_eq!(codes_paths(&named(&spec, dir.path())), hsh);
}

#[test]
fn a_damaged_meta_is_reported_not_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, _) = stored(dir.path(), b"abc");
    let meta = dir.path().join("store/meta").join(format!("{digest}.json"));
    let mut perm = std::fs::metadata(&meta).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&meta, perm).unwrap();
    std::fs::write(&meta, b"not json").unwrap();
    let d = named(
        &with_input("data/none.txt", &digest, "calibration"),
        dir.path(),
    );
    assert_eq!(codes_paths(&d), [("E-HSH-001", "inputs[0].sha256")]);
    assert!(d[0].problem.contains("cannot be read"), "{}", d[0].problem);
}

#[test]
fn a_pin_on_an_unsafe_path_is_left_to_the_path_rule() {
    let dir = tempfile::tempdir().unwrap();
    let d = named(&with_input("../d.txt", ABC, "calibration"), dir.path());
    assert_eq!(codes_paths(&d), [("E-PTH-001", "inputs[0].path")]);
}

#[test]
fn two_inputs_with_one_path_are_not_nested() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = with_input("data/a", "", "calibration");
    let again = s["inputs"][0].clone();
    s["inputs"].as_array_mut().unwrap().insert(1, again);
    assert!(named(&s, dir.path()).is_empty());
}

// ---- clusters: content, units and strata ----

/// `ok()` with its selection input at `sel` and its first confirmation input at `conf`.
fn pair(sel: &str, conf: &str) -> Value {
    let mut s = ok();
    set(&mut s, "/inputs/1/path", j(json!(sel)));
    set(&mut s, "/inputs/2/path", j(json!(conf)));
    s
}

#[test]
fn two_paths_with_the_same_bytes_are_not_disjoint() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/s.txt", b"one text");
    write(dir.path(), "data/c.txt", b"one text");
    write(dir.path(), "data/d.txt", b"another text");
    let d = named(&pair("data/s.txt", "data/c.txt"), dir.path());
    assert_eq!(codes_paths(&d), [("E-CNF-004", "inputs[2].sha256")]);
    assert_eq!(
        d[0].problem,
        "The plan has no confirmation input, or a confirmation input holds the same data as a selection or calibration input. It holds the data of inputs[1], a selection input."
    );
    assert!(named(&pair("data/s.txt", "data/d.txt"), dir.path()).is_empty());
    // A selection input that is not there shares nothing.
    assert!(named(&pair("data/none.txt", "data/c.txt"), dir.path()).is_empty());
    // Only confirmation inputs are flagged: a calibration input may hold a selection
    // input's data.
    let mut s = pair("data/d.txt", "data/c.txt");
    set(&mut s, "/inputs/0/path", j(json!("data/d.txt")));
    assert!(named(&s, dir.path()).is_empty());
}

#[test]
fn a_calibration_input_with_the_confirmation_data_is_not_disjoint() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/s.txt", b"selection text");
    write(dir.path(), "data/c.txt", b"one text");
    write(dir.path(), "data/k.txt", b"one text");
    write(dir.path(), "data/u.txt", b"ONE  TEXT");
    write(dir.path(), "data/d.txt", b"another text");
    let mut s = pair("data/s.txt", "data/c.txt");
    set(&mut s, "/inputs/0/path", j(json!("data/k.txt")));
    let d = named(&s, dir.path());
    assert_eq!(codes_paths(&d), [("E-CNF-004", "inputs[2].sha256")]);
    assert!(
        d[0].problem
            .ends_with(" It holds the data of inputs[0], a calibration input."),
        "{}",
        d[0].problem
    );
    // The same text in other case and spacing.
    set(&mut s, "/inputs/0/path", j(json!("data/u.txt")));
    let d = named(&s, dir.path());
    assert_eq!(codes_paths(&d), [("E-CNF-004", "inputs[2].sha256")]);
    // Other data passes.
    set(&mut s, "/inputs/0/path", j(json!("data/d.txt")));
    assert!(named(&s, dir.path()).is_empty());
    // A calibration input that is not there shares nothing.
    set(&mut s, "/inputs/0/path", j(json!("data/none.txt")));
    assert!(named(&s, dir.path()).is_empty());
}

#[test]
fn a_folder_input_holds_no_data_to_share() {
    // The box never reads or links a folder as an input, so a folder that holds a copy of
    // the other input's text shares nothing with it. One that holds the confirmation input
    // itself is nested, which the path rule reports.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/s.txt", b"selection text");
    write(dir.path(), "data/c.txt", b"one text");
    write(dir.path(), "cal/c.txt", b"one text");
    write(dir.path(), "data/k.txt", b"one text");
    let mut s = pair("data/s.txt", "data/c.txt");
    set(&mut s, "/inputs/0/path", j(json!("data/k.txt")));
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-CNF-004", "inputs[2].sha256")]
    );
    // The calibration input is a folder with the confirmation text inside.
    set(&mut s, "/inputs/0/path", j(json!("cal")));
    assert!(named(&s, dir.path()).is_empty());
    // The confirmation input is a folder with the calibration text inside.
    set(&mut s, "/inputs/0/path", j(json!("data/c.txt")));
    set(&mut s, "/inputs/2/path", j(json!("cal")));
    assert!(named(&s, dir.path()).is_empty());
    // The calibration input is the folder that holds the confirmation input.
    let mut s = pair("data/s.txt", "data/c.txt");
    set(&mut s, "/inputs/0/path", j(json!("data")));
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [
            ("E-PTH-001", "inputs[1].path"),
            ("E-PTH-001", "inputs[2].path")
        ]
    );
}

#[test]
fn an_exploratory_plan_may_calibrate_on_its_confirmation_text() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/s.txt", b"selection text");
    write(dir.path(), "data/c.txt", b"one text");
    write(dir.path(), "data/k.txt", b"one text");
    let mut s = pair("data/s.txt", "data/c.txt");
    set(&mut s, "/inputs/0/path", j(json!("data/k.txt")));
    assert_eq!(codes_paths(&named(&s, dir.path())).len(), 1);
    set(&mut s, "/study/mode", j(json!("exploratory")));
    assert!(
        named(&s, dir.path()).iter().all(|d| d.code != "E-CNF-004"),
        "{:?}",
        named(&s, dir.path())
    );
}

#[test]
fn a_case_and_space_copy_is_not_disjoint() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/s.txt", b"Alpha  Beta\n");
    write(dir.path(), "data/c.txt", b"alpha beta");
    let d = named(&pair("data/s.txt", "data/c.txt"), dir.path());
    assert_eq!(codes_paths(&d), [("E-CNF-004", "inputs[2].sha256")]);
}

#[test]
fn binary_inputs_without_a_normalized_hash_are_compared_by_their_bytes() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "data/s.bin", &[0xff, 0x00]);
    write(dir.path(), "data/c.bin", &[0xfe, 0x01]);
    write(dir.path(), "data/d.bin", &[0xff, 0x00]);
    assert!(named(&pair("data/s.bin", "data/c.bin"), dir.path()).is_empty());
    let d = named(&pair("data/s.bin", "data/d.bin"), dir.path());
    assert_eq!(codes_paths(&d), [("E-CNF-004", "inputs[2].sha256")]);
}

#[test]
fn a_stored_selection_input_is_compared_by_its_stored_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let (digest, _) = stored(dir.path(), b"Shared  Text");
    write(dir.path(), "data/c.txt", b"shared text");
    let mut s = pair("data/s.txt", "data/c.txt");
    set(&mut s, "/inputs/1/sha256", j(json!(digest)));
    let d = named(&s, dir.path());
    assert_eq!(codes_paths(&d), [("E-CNF-004", "inputs[2].sha256")]);
}

#[test]
fn a_confirmation_input_without_units_is_refused() {
    let dir = study_dir();
    let mut s = edited(&[("/inputs", j(inputs(36, 5)))]);
    set(&mut s, "/inputs/5/units", None);
    let d = named(&s, dir.path());
    assert_eq!(codes_paths(&d), [("E-UNT-001", "inputs[5].units")]);
    assert_eq!(
        d[0].problem,
        "The plan has no selection or confirmation input, or one does not declare its units, or its stratum while the design names strata."
    );
    // The same in exploratory mode.
    set(&mut s, "/study/mode", j(json!("exploratory")));
    set(&mut s, "/pins/deviations", None);
    let d = named(&s, dir.path());
    assert_eq!(codes_paths(&d), [("E-UNT-001", "inputs[5].units")]);
    // A mistyped count is the type rule's.
    set(&mut s, "/inputs/5/units", j(json!("five")));
    assert!(named(&s, dir.path()).is_empty());
}

#[test]
fn a_stratum_is_required_when_the_design_names_strata() {
    let dir = study_dir();
    // 37 clusters, so the 35 left in stratum fiction pass the power gate.
    let mut s = edited(&[("/inputs", j(inputs(37, 5)))]);
    set(&mut s, "/inputs/4/stratum", j(json!("")));
    set(&mut s, "/inputs/6/stratum", None);
    let want = [
        ("E-UNT-001", "inputs[4].stratum"),
        ("E-UNT-001", "inputs[6].stratum"),
    ];
    assert_eq!(codes_paths(&named(&s, dir.path())), want);
    set(&mut s, "/design/stratum", j(json!("")));
    assert!(named(&s, dir.path()).is_empty());
    // A mistyped stratum is the type rule's.
    set(&mut s, "/design/stratum", j(json!("genre")));
    set(&mut s, "/inputs/4/stratum", j(json!(7)));
    assert_eq!(
        codes_paths(&named(&s, dir.path())),
        [("E-UNT-001", "inputs[6].stratum")]
    );
}

#[test]
fn the_row_cap_counts_units_conditions_and_repeats_per_stage() {
    let dir = study_dir();
    // Exploratory: the power gate would refuse one confirmation cluster.
    let at = |upc: u64, units: u64, repeats: u64| {
        edited(&[
            ("/study/mode", j(json!("exploratory"))),
            UNDECLARED,
            ("/design/units_per_cluster", j(json!(upc))),
            ("/inputs", j(inputs(1, units))),
            ("/design/repeats", j(json!(repeats))),
        ])
    };
    let cap = [("E-UNT-002", "design.units_per_cluster")];
    // 20,000 selection units x (the reference + 2 candidates), then 20,000 confirmation
    // units x (the reference + the pick), 1 repeat: 100,000 rows.
    assert!(named(&at(20_000, 20_000, 1), dir.path()).is_empty());
    let over = at(20_001, 20_001, 1);
    let d = named(&over, dir.path());
    assert_eq!(codes_paths(&d), cap);
    assert_eq!(
        d[0].problem,
        "The schedule has more rows than the box allows. It would hold 100005 rows; the cap is 100000."
    );
    // The count is the count of the schedule `run` would write.
    let spec = ignorarium::spec::model::Spec::from_value(&over).unwrap();
    let rows = ignorarium::schedule::rows(&spec, &[0; 32], false).unwrap();
    assert_eq!(rows.len(), 100_005);
    // A cluster gives at most units_per_cluster, and at most its own units.
    assert!(named(&at(20_000, 4_000_000_000, 1), dir.path()).is_empty());
    assert!(named(&at(4_000_000_000, 20_000, 1), dir.path()).is_empty());
    // Repeats and each stage's conditions multiply; a condition no stage runs adds no row.
    let cap_only = |s: &Value| -> bool {
        let found = named(s, dir.path());
        codes_paths(&found)
            .into_iter()
            .filter(|c| c.0 != "E-CNF-003")
            .eq(cap.iter().copied())
    };
    assert!(cap_only(&at(20_000, 20_000, 2)));
    // The pilot also measured arm-c as the pick.
    let mut measured = pilot_estimates();
    measured.push(common::estimate(
        &[("arm-c", 1.0, json!({}))],
        "",
        4.0,
        Some(0.0),
    ));
    let grow = |stage: Option<&str>| {
        let mut s = at(20_000, 20_000, 1);
        let ids = s["design"]["conditions"].as_array_mut().unwrap();
        ids.push(json!({ "id": "arm-c" }));
        if let Some(stage) = stage {
            let list = s["design"][stage]["conditions"].as_array_mut().unwrap();
            list.push(json!("arm-c"));
        }
        let c = with_box(dir.path(), holding(measured.clone()));
        rules::check_named(&s, v0(), &c)
    };
    assert_eq!(codes_paths(&grow(Some("selection"))), cap);
    assert_eq!(codes_paths(&grow(Some("confirmation"))), cap);
    assert!(grow(None).is_empty());
    // Without a selection stage, the selection input runs nothing.
    let mut s = at(20_001, 50_000, 1);
    set(&mut s, "/design/selection", None);
    set(
        &mut s,
        "/design/confirmation/conditions",
        j(json!(["arm-a"])),
    );
    assert!(!codes_paths(&named(&s, dir.path())).contains(&cap[0]));
    // The largest counts do not overflow.
    let max = u64::from(u32::MAX);
    assert!(cap_only(&at(max, max, max)));
}

#[test]
fn the_template_selection_example_passes_once_uncommented() {
    let dir = study_dir();
    for t in ["numeric-ablation", "planted-spawn"] {
        let text = std::fs::read_to_string(format!("../../templates/{t}.toml")).unwrap();
        let (start, end) = ("# params = ", "# rule = \"best-mean\"");
        let from = text.find(start).unwrap();
        let to = from + text[from..].find(end).unwrap() + end.len();
        let block: Vec<&str> = text[from..to]
            .lines()
            .map(|l| l.strip_prefix("# ").unwrap())
            .collect();
        let text = format!("{}{}{}", &text[..from], block.join("\n"), &text[to..]);
        let mut v = ignorarium::spec::parse(text.as_bytes()).unwrap().value;
        // The stage needs a selection input, and its pick goes forward.
        set(&mut v, "/inputs/0/role", j(json!("selection")));
        set(&mut v, "/inputs/0/units", j(json!(3)));
        set(&mut v, "/inputs/0/stratum", j(json!("fiction")));
        set(
            &mut v,
            "/design/confirmation/conditions",
            j(json!(["selected"])),
        );
        set(
            &mut v,
            "/question/hypotheses/0/contrast",
            j(json!({ "selected": 1.0 })),
        );
        let (_, _, d) = rules::check_all(&v, v0(), &ctx(dir.path()));
        let errors: Vec<_> = d.iter().filter(|x| x.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "{t}: {errors:?}");
        assert!(v["design"]["selection"].is_object(), "{t}");
    }
}

// ---- conditions taken from an earlier study ----

const BITS: &str = "/design/conditions/2/params/bits";

/// `ok()` with arm-b's `bits` taken from study `chooser`, checked against `b`.
fn param_diags(mode: &str, b: common::Studies) -> Vec<Diagnostic> {
    let dir = study_dir();
    let spec = edited(&[
        ("/study/mode", j(json!(mode))),
        (BITS, j(json!({ "from": "chooser" }))),
    ]);
    rules::check_named(&spec, v0(), &with_box(dir.path(), b))
        .into_iter()
        .filter(|d| d.path.contains(".params."))
        .collect()
}

fn chooser(records: &[(&'static str, Value)]) -> common::Studies {
    common::Studies(vec![(
        "chooser",
        Ok(common::records_for("chooser", records)),
    )])
}

#[test]
fn a_param_from_an_unfinished_study_is_pending() {
    let finished = common::finished_selecting("arm-a", vec![common::arm_a()]);
    // Registered with no analysis record; the analysis record promises an estimate not there.
    let unfinished = || {
        [
            (common::Studies(vec![]), "not registered"),
            (chooser(&finished[..1]), "not finished"),
            (chooser(&finished[..2]), "not finished"),
            (chooser(&finished[..3]), "not finished"),
        ]
    };
    for mode in ["confirmatory", "exploratory"] {
        for (b, which) in unfinished() {
            let d = param_diags(mode, b);
            assert_eq!(
                codes_paths(&d),
                [("W-REF-001", "design.conditions[2].params.bits")],
                "{mode} {which}"
            );
            assert_eq!(d[0].severity, Severity::Warning);
            assert_eq!(
                d[0].problem,
                format!(
                    "A value waits on a study that has not finished. Waits on study chooser, which is {which}."
                )
            );
        }
        // Finished with a selection: nothing waits, and nothing is refused.
        assert!(param_diags(mode, chooser(&finished)).is_empty(), "{mode}");
    }
}

#[test]
fn a_param_from_a_study_without_selection_is_refused() {
    let none = common::finished(vec![common::arm_a()]);
    let path = [("E-REF-002", "design.conditions[2].params.bits.from")];
    for mode in ["confirmatory", "exploratory"] {
        let d = param_diags(mode, chooser(&none));
        assert_eq!(codes_paths(&d), path, "{mode}");
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(
            d[0].problem,
            "A reference names this study, or a study that recorded no selection. Study chooser recorded no selection."
        );
    }
    // A log the box cannot read refuses too.
    let broken = common::Studies(vec![("chooser", Err("bad signature".into()))]);
    let d = param_diags("confirmatory", broken);
    assert_eq!(codes_paths(&d), path);
    assert!(
        d[0].problem
            .ends_with("The box cannot read the records of study chooser: bad signature."),
        "{}",
        d[0].problem
    );
    // A study cannot take a condition from itself.
    let dir = study_dir();
    let own = edited(&[(BITS, j(json!({ "from": "my-study" })))]);
    let d = rules::check_named(&own, v0(), &ctx(dir.path()));
    let own: Vec<_> = d.iter().filter(|d| d.path.contains(".params.")).collect();
    assert_eq!(own.len(), 1);
    assert_eq!((own[0].code.as_str(), own[0].path.as_str()), path[0],);
    assert_eq!(
        own[0].problem,
        "A reference names this study, or a study that recorded no selection."
    );
}

#[test]
fn estimate_keys_match_across_studies_with_the_same_resolved_params() {
    // arm-b takes its bits from the condition `chooser` selected. The pilot measured arm-b with
    // bits = "arm-a".
    let measured = |bits: &str| {
        vec![common::estimate(
            &[("arm-b", 1.0, json!({ "bits": bits }))],
            "",
            4.0,
            Some(0.0),
        )]
    };
    let edits = unstaged(vec![
        (BITS, j(json!({ "from": "chooser" }))),
        ("/design/confirmation/conditions", j(json!(["arm-b"]))),
        hyp("contrast", j(json!({ "arm-b": 1.0 }))),
    ]);
    let boxed = |selected: &str, bits: &str| {
        common::Studies(vec![
            (
                "pilot",
                Ok(common::records_for(
                    "pilot",
                    &common::finished(measured(bits)),
                )),
            ),
            (
                "chooser",
                Ok(common::records_for(
                    "chooser",
                    &common::finished_selecting(selected, vec![]),
                )),
            ),
        ])
    };
    // The same resolved params match; another selection is another contrast.
    assert!(missing(&edits, boxed("arm-a", "arm-a")).is_empty());
    assert_eq!(missing(&edits, boxed("arm-b", "arm-a")).len(), 1);
    assert_eq!(missing(&edits, boxed("arm-a", "arm-b")).len(), 1);
    // A reference left pending gives no key to look for: W-REF-001 reports it alone.
    let waiting = common::Studies(vec![(
        "pilot",
        Ok(common::records_for(
            "pilot",
            &common::finished(measured("arm-a")),
        )),
    )]);
    assert!(missing(&edits, waiting).is_empty());
}

#[test]
fn an_estimates_study_that_has_not_finished_waits_while_a_param_waits_too() {
    // arm-b, the contrast's one term, takes its bits from `chooser`, which has not run: the
    // contrast's key is unknown, but a study that has not finished is pending whatever the key.
    let edits = unstaged(vec![
        (BITS, j(json!({ "from": "chooser" }))),
        ("/design/confirmation/conditions", j(json!(["arm-b"]))),
        hyp("contrast", j(json!({ "arm-b": 1.0 }))),
    ]);
    let waits = |b: common::Studies| -> Vec<(String, String)> {
        let dir = study_dir();
        rules::check_named(&edited(&edits), v0(), &with_box(dir.path(), b))
            .into_iter()
            .filter(|d| d.code == "W-REF-001")
            .map(|d| (d.path, d.problem))
            .collect()
    };
    let wait = |path: &str, study: &str, which: &str| {
        (
            path.to_string(),
            format!(
                "A value waits on a study that has not finished. Waits on study {study}, which is {which}."
            ),
        )
    };
    let bits = wait(
        "design.conditions[2].params.bits",
        "chooser",
        "not registered",
    );
    let pilot = common::finished(pilot_estimates());
    for (b, which) in [
        (common::Studies(vec![]), "not registered"),
        (
            common::Studies(vec![(
                "pilot",
                Ok(common::records_for("pilot", &pilot[..1])),
            )]),
            "not finished",
        ),
    ] {
        assert_eq!(
            waits(b),
            [
                wait("question.hypotheses[0].estimates", "pilot", which),
                bits.clone()
            ],
            "{which}"
        );
    }
    // A finished study waits on nothing; only the param does.
    let done = common::Studies(vec![("pilot", Ok(common::records_for("pilot", &pilot)))]);
    assert_eq!(waits(done), [bits]);
}

// ---- the code of a study the plan takes estimates from ----

const REF003: &str =
    "The plan takes estimates from a study that ran other code, and declares no deviation from it.";

/// The problem E-REF-003 states at a hypothesis whose study `pilot` ran `i`, when this plan
/// differs in `parts`: the inputs are listed when they differ, and `""` prints as `none`.
fn ran(i: &Instrument, parts: &str) -> String {
    let none = |x: &str| if x.is_empty() { "none" } else { x }.to_string();
    let listed: Vec<String> = (i.inputs.iter())
        .map(|(path, role, sha)| format!("{path} ({role}, {})", none(sha)))
        .collect();
    let inputs = match (parts.contains("inputs"), listed.is_empty()) {
        (false, _) => String::new(),
        (true, true) => ", and no model or calibration input".into(),
        (true, false) => format!(", and model and calibration inputs {}", listed.join(", ")),
    };
    format!(
        "{REF003} Study pilot ran payload {}, environment {}{inputs}; this plan differs in {parts}.",
        none(&i.payload),
        none(&i.environment)
    )
}

/// A box whose finished study `pilot` ran `instrument`.
fn ran_with(instrument: Instrument) -> common::Studies {
    let records = common::finished_with(instrument, None, pilot_estimates());
    common::Studies(vec![("pilot", Ok(common::records_for("pilot", &records)))])
}

/// The E-REF-003 diagnostics of the plan `edits` make, in a box whose `pilot` ran `instrument`.
fn ref003(dir: &Path, edits: &[Edit], instrument: Instrument) -> Vec<Diagnostic> {
    rules::check_named(&edited(edits), v0(), &with_box(dir, ran_with(instrument)))
        .into_iter()
        .filter(|d| d.code == "E-REF-003")
        .collect()
}

#[test]
fn matching_pins_need_no_deviation() {
    let dir = study_dir();
    let spec = edited(&matched(vec![]));
    assert!(named(&spec, dir.path()).is_empty());
    let analysis = hashes(pilot_estimates())[1].clone();
    assert_eq!(
        rules::borrowed(&spec, &ctx(dir.path())),
        [Borrowed {
            study: "pilot".into(),
            analysis: analysis.clone(),
            instrument: common::instrument(),
            recheck_passed: false,
            differs: vec![],
        }]
    );
    // The base plan pins none of pilot's code.
    assert_eq!(
        rules::borrowed(&ok(), &ctx(dir.path())),
        [Borrowed {
            study: "pilot".into(),
            analysis,
            instrument: common::instrument(),
            recheck_passed: false,
            differs: vec!["payload", "environment", "inputs"],
        }]
    );
}

#[test]
fn a_declared_deviation_clears_the_hit() {
    let dir = study_dir();
    let i = common::instrument();
    assert!(named(&ok(), dir.path()).is_empty());
    let d = named(&edited(&[("/pins/deviations", None)]), dir.path());
    assert_eq!(codes_paths(&d), [SAME]);
    assert_eq!(d[0].problem, ran(&i, "payload, environment, inputs"));
    // Each hypothesis that takes estimates from the study is reported, and one deviation
    // covers them all.
    let two = and(shared(&[0.5, 0.5]), vec![("/pins/deviations", None)]);
    assert_eq!(
        codes_paths(&named(&edited(&two), dir.path())),
        [SAME, ("E-REF-003", "question.hypotheses[1].estimates")]
    );
    assert!(named(&edited(&shared(&[0.5, 0.5])), dir.path()).is_empty());
    // A deviation from another study covers nothing, and names a study the plan takes no
    // estimates from.
    let d = named(
        &edited(&[("/pins/deviations/0/from", j(json!("nobody")))]),
        dir.path(),
    );
    assert_eq!(codes_paths(&d), [SAME, STRAY]);
    assert_eq!(
        d[1].problem,
        format!(
            "{REF003} Only a confirmatory hypothesis with a claim takes estimates, and none takes them from study nobody."
        )
    );
    // A hypothesis without a claim takes no estimates, whatever it names: it is not reported,
    // and a deviation from its study alone names a study the plan takes no estimates from.
    let mut p1 = descriptive("P1");
    p1["estimates"] = json!({ "from": "pilot" });
    let mut later = h1();
    later["estimates"] = json!({ "from": "later" });
    let reported = |edits: &[Edit]| -> Vec<(String, String)> {
        (named(&edited(edits), dir.path()).into_iter())
            .filter(|d| d.code == "E-REF-003")
            .map(|d| (d.code, d.path))
            .collect()
    };
    let both = ("/question/hypotheses", j(json!([h1(), p1.clone()])));
    assert_eq!(
        reported(&[both, ("/pins/deviations", None)]),
        [(SAME.0.to_string(), SAME.1.to_string())]
    );
    let apart = ("/question/hypotheses", j(json!([later, p1])));
    let declared = json!([deviation("later"), deviation("pilot")]);
    assert_eq!(
        reported(&[apart, ("/pins/deviations", j(declared))]),
        [(
            "E-REF-003".to_string(),
            "pins.deviations[1].from".to_string()
        )]
    );
}

#[test]
fn each_part_that_differs_is_named() {
    let dir = study_dir();
    let i = common::instrument();
    let other = "ab".repeat(32);
    let o = other.as_str();
    for (edits, parts) in [
        (vec![("/pins/payload", j(json!(o)))], "payload"),
        (vec![("/pins/environment", j(json!("")))], "environment"),
        (vec![("/pins/environment", j(json!(o)))], "environment"),
        // Another pin, path or role of the model input; another input of the code.
        (vec![("/inputs/0/sha256", j(json!(o)))], "inputs"),
        (vec![("/inputs/0/path", j(json!("model/n.bin")))], "inputs"),
        (vec![("/inputs/0/role", j(json!("calibration")))], "inputs"),
        (
            vec![
                ("/inputs/2", j(model_input())),
                ("/inputs/2/path", j(json!("model/n.bin"))),
            ],
            "inputs",
        ),
        (
            vec![
                ("/pins/payload", j(json!(o))),
                ("/inputs/0/sha256", j(json!(o))),
            ],
            "payload, inputs",
        ),
    ] {
        let d = ref003(dir.path(), &matched(edits.clone()), i.clone());
        assert_eq!(codes_paths(&d), [SAME], "{edits:?}");
        assert_eq!(d[0].problem, ran(&i, parts));
    }
    // The detail names the study's inputs, so the plan can pin them.
    let d = ref003(
        dir.path(),
        &matched(vec![("/inputs/0/sha256", j(json!(o)))]),
        i.clone(),
    );
    assert_eq!(
        d[0].problem,
        format!(
            "{REF003} Study pilot ran payload {}, environment {}, and model and calibration inputs model/m.bin (model, {}); this plan differs in inputs.",
            "cd".repeat(32),
            "ef".repeat(32),
            sha256_hex(common::MODEL)
        )
    );
    let d = ref003(
        dir.path(),
        &matched(vec![("/pins/payload", j(json!(o)))]),
        i.clone(),
    );
    assert!(!d[0].problem.contains("model/m.bin"), "{}", d[0].problem);
    // A path from the study's records prints on one line, in its order.
    let turned = Instrument {
        inputs: vec![("model/\u{202e}m.bin".into(), "model".into(), o.into())],
        ..i.clone()
    };
    let d = ref003(dir.path(), &matched(vec![]), turned);
    assert!(
        d[0].problem
            .contains(&format!("inputs model/\\u{{202e}}m.bin (model, {o});")),
        "{}",
        d[0].problem
    );
    // Selection and confirmation inputs are data, not code.
    let data = matched(vec![("/inputs/2/sha256", j(json!(o)))]);
    assert!(ref003(dir.path(), &data, i.clone()).is_empty());
    // The study's inputs are compared as a set, whatever order the plan lists them in.
    let two = Instrument {
        inputs: vec![
            ("model/a.bin".into(), "model".into(), other.clone()),
            (
                "model/m.bin".into(),
                "model".into(),
                sha256_hex(common::MODEL),
            ),
        ],
        ..i.clone()
    };
    let mut second = model_input();
    second["path"] = json!("model/a.bin");
    second["sha256"] = json!(o);
    let listed = matched(vec![("/inputs/2", j(second))]);
    assert!(ref003(dir.path(), &listed, two.clone()).is_empty());
    let missing = ref003(dir.path(), &matched(vec![]), two.clone());
    assert_eq!(missing[0].problem, ran(&two, "inputs"));
    // An empty pin pins nothing, even where the study names no digest.
    let unpinned = Instrument {
        inputs: vec![("model/m.bin".into(), "model".into(), String::new())],
        ..i.clone()
    };
    let empty = matched(vec![("/inputs/0/sha256", j(json!("")))]);
    let d = ref003(dir.path(), &empty, unpinned.clone());
    assert_eq!(d[0].problem, ran(&unpinned, "inputs"));
}

#[test]
fn a_source_without_one_environment_needs_a_deviation() {
    let dir = study_dir();
    let i = common::instrument();
    // Rows in two environments name none; rows on the host name no image a pin can name.
    for (theirs, shown) in [("", "none"), ("host", "host")] {
        for pin in [i.environment.as_str(), theirs] {
            let source = Instrument {
                environment: theirs.into(),
                ..i.clone()
            };
            let edits = matched(vec![("/pins/environment", j(json!(pin)))]);
            let d = ref003(dir.path(), &edits, source.clone());
            assert_eq!(codes_paths(&d), [SAME], "{theirs:?} {pin:?}");
            assert_eq!(d[0].problem, ran(&source, "environment"));
            assert!(d[0].problem.contains(&format!(", environment {shown};")));
        }
    }
    // A study recorded before its analysis named the code differs in every part.
    let d = ref003(dir.path(), &matched(vec![]), Instrument::default());
    assert_eq!(
        d[0].problem,
        format!(
            "{REF003} Study pilot ran payload none, environment none, and no model or calibration input; this plan differs in payload, environment, inputs."
        )
    );
}

#[test]
fn an_empty_payload_pin_stands_for_the_payload_folder() {
    let dir = study_dir();
    write(dir.path(), "payload/run", b"#!/bin/sh\n");
    let folder = ignorarium::payload::tree_hash(&dir.path().join("payload")).unwrap();
    let source = Instrument {
        payload: folder.clone(),
        ..common::instrument()
    };
    let pinned = |pin: &str| {
        ref003(
            dir.path(),
            &matched(vec![("/pins/payload", j(json!(pin)))]),
            source.clone(),
        )
    };
    assert!(pinned("").is_empty());
    assert!(pinned(&folder).is_empty());
    // A pin is the payload the plan names, whatever the folder holds.
    assert_eq!(codes_paths(&pinned(&"ab".repeat(32))), [SAME]);
    write(dir.path(), "payload/run", b"#!/bin/sh\nexit 1\n");
    assert_eq!(codes_paths(&pinned("")), [SAME]);
    assert!(pinned(&folder).is_empty());
}

#[test]
fn one_check_hashes_the_payload_folder_once() {
    let dir = study_dir();
    write(dir.path(), "payload/run", b"#!/bin/sh\n");
    let folder = ignorarium::payload::tree_hash(&dir.path().join("payload")).unwrap();
    let source = Instrument {
        payload: folder.clone(),
        ..common::instrument()
    };
    let (pilot, analysis) = checked("pilot", source, Some(passed_check()));
    let check = || with_box(dir.path(), common::Studies(vec![pilot.clone()]));
    let spec = edited(&steady(vec![("/pins/payload", j(json!("")))]));
    let named = |c: &CheckCtx| rules::check_named(&spec, v0(), c);
    let c = check();
    assert_eq!(rules::carried(&spec, &c), carry("pilot", &analysis));
    // The folder changes after the carry hashed it. The rest of this check, and the register
    // that freezes its hash, compare the code the carry compared.
    write(dir.path(), "payload/run", b"#!/bin/sh\nexit 1\n");
    let none: [(&str, &str); 0] = [];
    assert_eq!(codes_paths(&named(&c)), none);
    assert_eq!(rules::borrowed(&spec, &c)[0].differs, Vec::<&str>::new());
    assert_eq!(rules::carried(&spec, &c), carry("pilot", &analysis));
    assert_eq!(c.payload_hash().as_ref().unwrap(), &folder);
    // A new check reads the folder as it is now.
    let fresh = check();
    assert_eq!(codes_paths(&named(&fresh)), [SAME]);
    assert_eq!(rules::borrowed(&spec, &fresh)[0].differs, ["payload"]);
    assert_eq!(rules::carried(&spec, &fresh), None);
}

#[test]
fn a_pending_source_is_left_to_w_ref_001() {
    let dir = study_dir();
    let begun = common::records_for("pilot", &common::finished(pilot_estimates())[..1]);
    let wait = ("W-REF-001", "question.hypotheses[0].estimates");
    let unpinned = edited(&[("/pins/deviations", None)]);
    for registered in [false, true] {
        let held = if registered {
            vec![("pilot", Ok(begun.clone()))]
        } else {
            vec![]
        };
        let c = with_box(dir.path(), common::Studies(held));
        assert_eq!(
            codes_paths(&rules::check_named(&unpinned, v0(), &c)),
            [wait],
            "{registered}"
        );
        assert!(rules::borrowed(&unpinned, &c).is_empty());
        // A deviation from the study it waits on names a study it takes estimates from.
        assert_eq!(
            codes_paths(&rules::check_named(&ok(), v0(), &c)),
            [wait],
            "{registered}"
        );
    }
    // A record log that cannot be read is E-CNF-003's.
    let torn = common::Studies(vec![("pilot", Err("line 2 is not a whole record".into()))]);
    let c = with_box(dir.path(), torn);
    assert_eq!(
        codes_paths(&rules::check_named(&unpinned, v0(), &c)),
        [("E-CNF-003", "question.hypotheses[0].estimates")]
    );
    assert!(rules::borrowed(&unpinned, &c).is_empty());
}

#[test]
fn the_studies_borrowed_from_are_the_finished_ones_each_once_first_mention_first() {
    let dir = study_dir();
    let claimed = |id: &str, from: &str| {
        let mut h = h1();
        h["id"] = json!(id);
        h["estimates"] = json!({ "from": from });
        h["alpha_share"] = json!(0.25);
        h
    };
    let mut described = descriptive("P1");
    described["estimates"] = json!({ "from": "late" });
    let spec = edited(&[(
        "/question/hypotheses",
        j(json!([
            claimed("H1", "early"),
            claimed("H2", "pilot"),
            claimed("H3", "early"),
            claimed("H4", "pending"),
            described,
        ])),
    )]);
    let early = Instrument {
        payload: "aa".repeat(32),
        ..common::instrument()
    };
    let run = |id: &'static str, i: Instrument| {
        let records = common::records_for(id, &common::finished_with(i, None, pilot_estimates()));
        let analysis = records::hash(&records[1]);
        ((id, Ok::<_, String>(records)), analysis)
    };
    let (e, early_hash) = run("early", early.clone());
    let (p, pilot_hash) = run("pilot", common::instrument());
    let (l, _) = run("late", common::instrument());
    let pending = (
        "pending",
        Ok(common::records_for(
            "pending",
            &common::finished(vec![])[..1],
        )),
    );
    let c = with_box(dir.path(), common::Studies(vec![l, pending, p, e]));
    let parts = vec!["payload", "environment", "inputs"];
    assert_eq!(
        rules::borrowed(&spec, &c),
        [
            Borrowed {
                study: "early".into(),
                analysis: early_hash,
                instrument: early,
                recheck_passed: false,
                differs: parts.clone(),
            },
            Borrowed {
                study: "pilot".into(),
                analysis: pilot_hash,
                instrument: common::instrument(),
                recheck_passed: false,
                differs: parts,
            },
        ]
    );
    // An exploratory plan takes no estimates.
    let mut s = spec.clone();
    s["study"]["mode"] = json!("exploratory");
    assert!(rules::borrowed(&s, &c).is_empty());
}

#[test]
fn a_deviation_names_a_study_a_reason_and_a_known_timing() {
    type Found = Vec<(String, String)>;
    let dir = study_dir();
    let at = |edits: Vec<Edit>| -> Found {
        let (_, _, d) = rules::check_all(&edited(&edits), v0(), &ctx(dir.path()));
        (d.into_iter())
            .filter(|x| x.path.starts_with("pins"))
            .map(|x| (x.code, x.path))
            .collect()
    };
    let one = |code: &str, path: &str| vec![(code.to_string(), path.to_string())];
    const T: &str = "/pins/deviations/0/timing";
    for timing in [
        "before-collection",
        "during-collection",
        "after-collection-before-access",
        "after-access-before-results",
        "after-results",
    ] {
        assert!(at(vec![(T, j(json!(timing)))]).is_empty(), "{timing}");
    }
    assert!(at(vec![]).is_empty());
    let cases: Vec<(Vec<Edit>, Found)> = vec![
        (
            vec![(T, j(json!("later")))],
            one("E-ENM-001", "pins.deviations[0].timing"),
        ),
        (
            vec![(T, j(json!(3)))],
            one("E-TYP-001", "pins.deviations[0].timing"),
        ),
        (
            vec![(T, None)],
            one("E-REQ-001", "pins.deviations[0].timing"),
        ),
        (
            vec![("/pins/deviations/0/reason", j(json!("")))],
            one("E-REF-003", "pins.deviations[0].reason"),
        ),
        (
            vec![("/pins/deviations/0/reason", j(json!(["why"])))],
            one("E-TYP-001", "pins.deviations[0].reason"),
        ),
        (
            vec![("/pins/deviations/0/reason", None)],
            [
                one("E-REQ-001", "pins.deviations[0].reason"),
                one("E-REF-003", "pins.deviations[0].reason"),
            ]
            .concat(),
        ),
        (
            vec![("/pins/deviations/0/from", None)],
            one("E-REQ-001", "pins.deviations[0].from"),
        ),
        (
            vec![("/pins/deviations/0/from", j(json!(3)))],
            one("E-TYP-001", "pins.deviations[0].from"),
        ),
        (
            vec![("/pins/deviations/0/from", j(json!("Pilot")))],
            one("E-FMT-001", "pins.deviations[0].from"),
        ),
        (
            vec![("/pins/deviations/0/note", j(json!("x")))],
            one("E-KEY-001", "pins.deviations[0].note"),
        ),
        (
            vec![("/pins/deviations", j(json!("pilot")))],
            one("E-TYP-001", "pins.deviations"),
        ),
        (
            vec![("/pins/deviations", j(json!(["pilot"])))],
            one("E-TYP-001", "pins.deviations[0]"),
        ),
        (
            vec![(
                "/pins/deviations",
                j(json!([deviation("pilot"), deviation("pilot")])),
            )],
            one("E-UNQ-001", "pins.deviations[1].from"),
        ),
        // A plan with no deviation lists none.
        (vec![("/pins/deviations", j(json!([])))], vec![]),
    ];
    for (edits, want) in cases {
        assert_eq!(at(edits.clone()), want, "{edits:?}");
    }
    let rule = v0().iter().find(|r| r.code == "E-REF-003").unwrap();
    assert_eq!(
        rule.instruction,
        "Pin the source study's payload, environment and model and calibration inputs, or add a `[[pins.deviations]]` entry with `from`, `reason` and `timing`. A deviation names a study that a confirmatory hypothesis with a claim takes estimates from, and gives a reason."
    );
}

// ---- the determinism check a plan carries ----

/// A recheck whose two copies matched their originals.
fn passed_check() -> Recheck {
    Recheck {
        rows: 2,
        mismatched: vec![],
        uncompared: vec![],
        carried_from: None,
    }
}

/// A study in a hand-built box: its id and its records.
type Held = (&'static str, Result<Vec<records::Record>, String>);

/// The finished study `id` that ran `instrument` with `recheck`, and the hash of its
/// analysis record.
fn checked(id: &'static str, instrument: Instrument, recheck: Option<Recheck>) -> (Held, String) {
    let finished = common::finished_with(instrument, recheck, pilot_estimates());
    let records = common::records_for(id, &finished);
    let analysis = records::hash(&records[1]);
    ((id, Ok(records)), analysis)
}

/// `matched` with `pins.deterministic`, then `extra`.
fn steady(extra: Vec<Edit>) -> Vec<Edit> {
    matched(and(vec![("/pins/deterministic", j(json!(true)))], extra))
}

fn carry(study: &str, record: &str) -> Option<Carried> {
    Some(Carried {
        study: study.into(),
        record: record.into(),
    })
}

#[test]
fn a_borrower_with_other_inputs_still_carries() {
    let dir = study_dir();
    let i = common::instrument();
    let (pilot, analysis) = checked("pilot", i.clone(), Some(passed_check()));
    let c = with_box(dir.path(), common::Studies(vec![pilot]));
    let spec = edited(&steady(vec![]));
    let found = Borrowed {
        study: "pilot".into(),
        analysis: analysis.clone(),
        instrument: i.clone(),
        recheck_passed: true,
        differs: vec![],
    };
    assert_eq!(rules::borrowed(&spec, &c), [found]);
    assert_eq!(rules::carried(&spec, &c), carry("pilot", &analysis));
    // Other model or calibration inputs need a deviation, and the code that ran is the same.
    let other = Instrument {
        inputs: vec![],
        ..i.clone()
    };
    let declared = steady(vec![("/pins/deviations", j(json!([deviation("pilot")])))]);
    let (pilot, analysis) = checked("pilot", other, Some(passed_check()));
    let c = with_box(dir.path(), common::Studies(vec![pilot]));
    let spec = edited(&declared);
    assert_eq!(rules::borrowed(&spec, &c)[0].differs, ["inputs"]);
    assert!(rules::check_named(&spec, v0(), &c).is_empty());
    assert_eq!(rules::carried(&spec, &c), carry("pilot", &analysis));
}

#[test]
fn only_a_deterministic_plan_whose_rows_run_in_the_pinned_environment_carries() {
    let dir = study_dir();
    let (pilot, analysis) = checked("pilot", common::instrument(), Some(passed_check()));
    let c = with_box(dir.path(), common::Studies(vec![pilot]));
    let carried = |edits: Vec<Edit>| rules::carried(&edited(&edits), &c);
    for level in [1, 2, 3] {
        let at = steady(vec![("/scope/min_isolation", j(json!(level)))]);
        assert_eq!(carried(at), carry("pilot", &analysis), "{level}");
    }
    // A plan that runs no recheck carries none.
    assert_eq!(carried(matched(vec![])), None);
    let unset = steady(vec![("/pins/deterministic", j(json!(false)))]);
    assert_eq!(carried(unset), None);
    // Level 0 runs rows on the host, whatever pins.environment names, and min_isolation 0
    // lets a host without level 1 run them there.
    let host = steady(vec![("/scope/min_isolation", j(json!(0)))]);
    assert_eq!(carried(host), None);
    // An exploratory plan takes no estimates.
    let exploring = steady(vec![("/study/mode", j(json!("exploratory")))]);
    assert_eq!(carried(exploring), None);
}

#[test]
fn a_borrower_with_another_payload_runs_its_own_rechecks() {
    let dir = study_dir();
    let i = common::instrument();
    let other = "ab".repeat(32);
    let o = other.as_str();
    let ran = |payload: &str, environment: &str| Instrument {
        payload: payload.into(),
        environment: environment.into(),
        ..i.clone()
    };
    let (p, e) = (i.payload.as_str(), i.environment.as_str());
    for (edits, source) in [
        // Another payload pinned, or another payload run.
        (vec![("/pins/payload", j(json!(o)))], ran(p, e)),
        (vec![], ran(o, e)),
        // Another environment, none pinned, or a study whose rows ran on the host or in two
        // environments.
        (vec![("/pins/environment", j(json!(o)))], ran(p, e)),
        (vec![("/pins/environment", j(json!("")))], ran(p, e)),
        (
            vec![("/pins/environment", j(json!("host")))],
            ran(p, "host"),
        ),
        (vec![("/pins/environment", j(json!("")))], ran(p, "")),
    ] {
        let declared = and(
            edits,
            vec![("/pins/deviations", j(json!([deviation("pilot")])))],
        );
        let (pilot, _) = checked("pilot", source.clone(), Some(passed_check()));
        let c = with_box(dir.path(), common::Studies(vec![pilot]));
        let found = rules::carried(&edited(&steady(declared.clone())), &c);
        assert_eq!(found, None, "{declared:?} {source:?}");
    }
    // A plan that takes estimates from two studies carries the check of the first that ran
    // its code.
    let claimed = |id: &str, from: &str| {
        let mut h = h1();
        h["id"] = json!(id);
        h["estimates"] = json!({ "from": from });
        h["alpha_share"] = json!(0.5);
        h
    };
    let both = |first: &str, second: &str| {
        let hs = json!([claimed("H1", first), claimed("H2", second)]);
        let ds = json!([deviation(first), deviation(second)]);
        edited(&steady(vec![
            ("/question/hypotheses", j(hs)),
            ("/pins/deviations", j(ds)),
        ]))
    };
    let (early, _) = checked("early", ran(o, e), Some(passed_check()));
    let (pilot, pilot_hash) = checked("pilot", ran(p, e), Some(passed_check()));
    let (late, late_hash) = checked("late", ran(p, e), Some(passed_check()));
    let c = with_box(dir.path(), common::Studies(vec![early, pilot, late]));
    let carried = |first: &str, second: &str| rules::carried(&both(first, second), &c);
    assert_eq!(carried("early", "pilot"), carry("pilot", &pilot_hash));
    assert_eq!(carried("late", "pilot"), carry("late", &late_hash));
}

#[test]
fn a_failed_check_is_not_carried() {
    let dir = study_dir();
    let spec = edited(&steady(vec![]));
    let failed = [
        // A study that declared no determinism, one whose copy differed, one whose copy was
        // not compared, and one that compared nothing.
        None,
        Some(Recheck {
            mismatched: vec![7],
            ..passed_check()
        }),
        Some(Recheck {
            rows: 1,
            uncompared: vec![6],
            ..passed_check()
        }),
        Some(Recheck {
            rows: 0,
            ..passed_check()
        }),
    ];
    for recheck in failed {
        let (pilot, _) = checked("pilot", common::instrument(), recheck.clone());
        let c = with_box(dir.path(), common::Studies(vec![pilot]));
        let b = rules::borrowed(&spec, &c);
        assert_eq!((b.len(), b[0].recheck_passed), (1, false), "{recheck:?}");
        assert_eq!(rules::carried(&spec, &c), None, "{recheck:?}");
    }
}

#[test]
fn a_study_that_carried_passes_it_on() {
    let dir = study_dir();
    let carried = Recheck {
        rows: 0,
        carried_from: Some("first".into()),
        ..passed_check()
    };
    let (pilot, analysis) = checked("pilot", common::instrument(), Some(carried));
    let c = with_box(dir.path(), common::Studies(vec![pilot]));
    let spec = edited(&steady(vec![]));
    assert!(rules::borrowed(&spec, &c)[0].recheck_passed);
    assert_eq!(rules::carried(&spec, &c), carry("pilot", &analysis));
}
