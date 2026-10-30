//! Typed spec schema. Shape only: value checks live in the rules.
use serde::Deserialize;
use serde_json::Value;

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
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub id: String,
    pub unit: String,
    pub direction: Direction,
    pub registered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
    pub test: TestKind,
    pub alpha: f64,
    pub power: f64,
    pub sesoi: f64,
    pub icc: f64,
    /// Units each cluster gives the study, at most.
    pub units_per_cluster: u32,
    pub pick: Pick,
    pub variance: Variance,
    pub conditions: Vec<Condition>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestKind {
    PairedMean,
    Descriptive,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variance {
    pub value: f64,
    /// "stated" or "record:<sha256>"; the rules check the format.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub id: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
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
