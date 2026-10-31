//! Typed spec schema. Shape only: value checks live in the rules.
use evidence::Tail;
use evidence::claim::{Bound, Statistic};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The condition id that stands for the selection stage's pick.
pub const SELECTED: &str = "selected";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub study: Study,
    pub question: Question,
    pub outcomes: Vec<Outcome>,
    pub scope: Scope,
    pub design: Design,
    pub inputs: Vec<Input>,
    pub pins: Pins,
    pub mutations: Mutations,
    pub access: Access,
    pub report: Report,
}

impl Spec {
    /// Call only after `check` passes.
    pub fn from_value(v: &Value) -> Result<Spec, serde_json::Error> {
        Spec::deserialize(v)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Study {
    pub id: String,
    pub template: Template,
    pub mode: Mode,
    pub label: Label,
    pub test_log: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Template {
    NumericAblation,
    PlantedSpawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Exploratory,
    Confirmatory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Label {
    Signal,
    Confirmed,
    Replicated,
    Generalized,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub population: String,
    pub intervention: String,
    pub comparator: String,
    pub negative_control: String,
    pub hypotheses: Vec<Hypothesis>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    pub rival: String,
    /// The outcome id whose paired values the contrast weighs.
    pub outcome: String,
    /// Condition id to weight: per unit, the weighted sum of the conditions' paired values.
    /// `selected` names the selection stage's pick.
    pub contrast: BTreeMap<String, f64>,
    /// The strata the claim must hold in, each on its own clusters; empty: every confirmation
    /// unit as one pooled group.
    #[serde(default)]
    pub strata: Vec<String>,
    /// `None`: a descriptive hypothesis, which gives estimates and no verdict.
    pub claim: Option<Claim>,
    pub statistic: Option<StatisticSpec>,
    /// The true value of the statistic where the test must have its power.
    pub power_at: Option<f64>,
    /// The study whose estimate records give the contrast's sd and icc.
    pub estimates: Option<Reference>,
    /// This hypothesis's share of the study's level.
    pub alpha_share: Option<f64>,
}

/// A one-key table: `{ below = m }`, `{ above = m }` or `{ within = [a, b] }`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Claim {
    Below(f64),
    Above(f64),
    Within([f64; 2]),
}

impl Claim {
    /// The one-sided bounds that must all hold: `within [a, b]` is above `a` and below `b`.
    pub fn bounds(&self) -> Vec<Bound> {
        let bound = |tail, margin| Bound { tail, margin };
        match *self {
            Claim::Below(m) => vec![bound(Tail::Lower, m)],
            Claim::Above(m) => vec![bound(Tail::Upper, m)],
            Claim::Within([a, b]) => vec![bound(Tail::Upper, a), bound(Tail::Lower, b)],
        }
    }
}

/// A one-key table: `{ mean = [lo, hi] }`, the range each unit is clipped to before a cluster
/// mean is taken, or `{ quantile = q }`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatisticSpec {
    Mean([f64; 2]),
    Quantile(f64),
}

impl StatisticSpec {
    pub fn to_evidence(&self) -> Statistic {
        match *self {
            StatisticSpec::Mean([lo, hi]) => Statistic::Mean { lo, hi },
            StatisticSpec::Quantile(q) => Statistic::Quantile(q),
        }
    }
}

/// A value another study gives: `{ from = "<study id>" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub id: String,
    pub unit: String,
    pub direction: Direction,
    /// The key the payload writes this outcome's value under in `values.json`.
    pub measure: String,
    /// How the box pairs a condition's value with the reference condition's.
    pub paired: Paired,
    pub registered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Paired {
    /// Condition minus reference.
    Difference,
    /// 100 x (condition / reference - 1).
    Percent,
    /// ln(condition / reference).
    LogRatio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Lower,
    Higher,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub min_isolation: u8,
    pub stop_rule: StopRule,
    pub budget: Budget,
    pub end_of_run: EndOfRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopRule {
    BudgetOnly,
    EProcess,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub cpu_hours: f64,
    pub wall_hours: f64,
    pub money_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndOfRun {
    pub route: Route,
    pub follow_on: Vec<FollowOn>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    Report,
    FollowOn,
    Share,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowOn {
    pub when: String,
    pub spec: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Design {
    pub unit: String,
    /// "" when none.
    pub cluster: String,
    /// "" when none.
    pub stratum: String,
    pub reference: String,
    pub repeats: u32,
    pub row_seconds: f64,
    /// The payload's data limit in MiB at every level; `None` when the plan declares none.
    #[serde(default)]
    pub row_memory_mib: Option<u32>,
    pub power: f64,
    /// Units each cluster gives the study, at most.
    pub units_per_cluster: u32,
    pub pick: Pick,
    pub conditions: Vec<Condition>,
    /// The stage that picks one candidate on the selection inputs; `None` when there is none.
    pub selection: Option<Selection>,
    pub confirmation: Stage,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    /// The outcome whose mean paired value ranks the candidates.
    pub metric: String,
    /// The candidates; a tie goes to the first listed.
    pub conditions: Vec<String>,
    pub rule: SelectionRule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionRule {
    /// The best mean, in the metric outcome's direction.
    BestMean,
}

/// The conditions a stage runs besides the reference, which runs in every stage.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub conditions: Vec<String>,
}

impl Stage {
    /// The stage runs the selection stage's pick.
    pub fn runs_pick(&self) -> bool {
        self.conditions.iter().any(|c| c == SELECTED)
    }
}

/// Which of a cluster's units the study takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pick {
    /// Drawn without replacement from the seed `units/<input path>`.
    Seeded,
    /// Indices 0, 1, 2, …
    First,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub id: String,
    /// What the payload gets, as JSON, in `IGNORARIUM_PARAMS`.
    #[serde(default)]
    pub params: BTreeMap<String, Param>,
}

/// Where a condition's param is named in the plan: the path `register` freezes its value under.
pub fn param_path(condition: usize, key: &str) -> String {
    format!("design.conditions[{condition}].params.{key}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Param {
    /// The condition the named study selected, frozen at register.
    Ref(Reference),
    Value(Scalar),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub path: String,
    pub sha256: String,
    pub role: Role,
    /// The stratum of this cluster; "" when the design names none.
    #[serde(default)]
    pub stratum: String,
    /// The units the file holds. Selection and confirmation inputs declare it.
    pub units: Option<u32>,
    pub licence: Licence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Calibration,
    Selection,
    Confirmation,
    Model,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Licence {
    pub spdx: String,
    #[serde(rename = "type")]
    pub r#type: LicenceType,
    pub source: String,
    pub creators: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LicenceType {
    Common,
    Custom,
    RequestForm,
    Unspecified,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pins {
    pub environment: String,
    pub payload: String,
    pub rule_base: String,
    /// One per study the plan takes estimates from without pinning that study's code.
    #[serde(default)]
    pub deviations: Vec<PinDeviation>,
    /// The payload gives the same output for the same job, so the box reruns one row per
    /// condition and compares.
    #[serde(default)]
    pub deterministic: bool,
}

/// A plan's departure from the code of a study it takes estimates from: why, and when.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinDeviation {
    /// The study id.
    pub from: String,
    pub reason: String,
    pub timing: Timing,
}

/// When a deviation happened, against the confirmation data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Timing {
    BeforeCollection,
    DuringCollection,
    AfterCollectionBeforeAccess,
    AfterAccessBeforeResults,
    AfterResults,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutations {
    pub depth_cap: u8,
    pub operators: Vec<Operator>,
    pub triggers: Vec<Trigger>,
    pub seeds: Seeds,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operator {
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: OperatorType,
    pub range: Vec<Scalar>,
    pub acts_on: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperatorType {
    Int,
    Float,
    Enum,
}

/// Integers come first so whole numbers stay integers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Int(i64),
    Float(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub id: String,
    pub metric: String,
    pub threshold_e: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seeds {
    pub spawn: String,
    pub report: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub blind: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub licence_code: String,
    pub licence_data: String,
    pub publish: Publish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Publish {
    None,
    ZenodoSandbox,
}
