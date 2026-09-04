//! Retrieval evaluation: a committed set of representative queries with
//! expected results and rank thresholds, run against a real index.
//! Retrieval quality is thereby a testable property, not intuition.

use knowledge_core::{KnowledgeRetriever, SearchQuery};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct EvalCase {
    /// The query text, as a coding agent would phrase it.
    pub text: String,
    /// Category: known_symbol, api_discovery, conceptual, cross_package,
    /// version_sensitive.
    pub category: String,
    /// At least one of these contexts (symbol path or joined section path)
    /// must appear in the top results. Prefix matching is used so a whole
    /// module can be an acceptable answer.
    pub expect_any: Vec<String>,
    /// Optional: restrict acceptable hits to this package ("name" or
    /// "name@version").
    pub expect_package: Option<String>,
    /// The first useful hit must rank at most this (1-based). Default 3.
    #[serde(default = "default_max_rank")]
    pub max_rank: usize,
}

fn default_max_rank() -> usize {
    3
}

/// Parses an eval TOML file ("[[case]]" array) into cases.
pub fn parse_cases(raw: &str) -> Result<Vec<EvalCase>, String> {
    #[derive(Deserialize)]
    struct EvalFile {
        case: Vec<EvalCase>,
    }
    let file: EvalFile = toml::from_str(raw).map_err(|e| e.to_string())?;
    Ok(file.case)
}

/// Result of one eval case.
#[derive(Clone, Debug)]
pub struct EvalOutcome {
    pub case: EvalCase,
    /// 1-based rank of the first useful hit, or None.
    pub rank: Option<usize>,
    /// Contexts of the top results, for diagnosis.
    pub top: Vec<String>,
    pub limit: usize,
}

impl EvalOutcome {
    pub fn passed(&self) -> bool {
        matches!(self.rank, Some(rank) if rank <= self.case.max_rank)
    }
}

/// Runs the cases against a retriever and returns per-case outcomes.
pub fn run_eval(retriever: &dyn KnowledgeRetriever, cases: Vec<EvalCase>) -> Vec<EvalOutcome> {
    let mut outcomes = Vec::with_capacity(cases.len());
    for case in cases {
        let limit = case.max_rank.clamp(5, 10);
        let query = SearchQuery {
            text: case.text.clone(),
            packages: Vec::new(),
            source_kinds: Vec::new(),
            item_kinds: Vec::new(),
            limit,
        };
        let hits = retriever.search(&query).unwrap_or_default();
        let top: Vec<String> = hits
            .iter()
            .map(|h| {
                let context = match &h.symbol_path {
                    Some(symbol) => symbol.clone(),
                    None if !h.section_path.is_empty() => h.section_path.join(" > "),
                    None => h.title.clone(),
                };
                if let Some(expected_package) = &case.expect_package {
                    let hit_key = format!("{}@{}", h.package_name, h.package_version);
                    let ok = match expected_package.split_once('@') {
                        Some((name, version)) => {
                            h.package_name == name && h.package_version == version
                        }
                        None => h.package_name == *expected_package || hit_key == *expected_package,
                    };
                    if ok {
                        format!("{context} [{}]", h.package_name)
                    } else {
                        format!("{context} [{}@{}]", h.package_name, h.package_version)
                    }
                } else {
                    context
                }
            })
            .collect();

        let rank = hits
            .iter()
            .position(|h| hit_matches(h, &case))
            .map(|i| i + 1);
        outcomes.push(EvalOutcome {
            case,
            rank,
            top,
            limit,
        });
    }
    outcomes
}

fn hit_matches(hit: &knowledge_core::SearchHit, case: &EvalCase) -> bool {
    let context = match &hit.symbol_path {
        Some(symbol) => symbol.clone(),
        None if !hit.section_path.is_empty() => hit.section_path.join(" > "),
        None => hit.title.clone(),
    };
    if let Some(expected_package) = &case.expect_package {
        let ok = match expected_package.split_once('@') {
            Some((name, version)) => hit.package_name == name && hit.package_version == version,
            None => hit.package_name == *expected_package,
        };
        if !ok {
            return false;
        }
    }
    case.expect_any
        .iter()
        .any(|expected| context == *expected || context.starts_with(expected))
}

/// Summary of an eval run.
#[derive(Clone, Debug)]
pub struct EvalSummary {
    pub total: usize,
    pub passed: usize,
    /// Mean reciprocal rank over all cases (0 if nothing found).
    pub mrr: f64,
    pub failures: Vec<EvalOutcome>,
}

pub fn summarize(outcomes: &[EvalOutcome]) -> EvalSummary {
    let total = outcomes.len();
    let passed = outcomes.iter().filter(|o| o.passed()).count();
    let mrr = if outcomes.is_empty() {
        0.0
    } else {
        outcomes
            .iter()
            .map(|o| o.rank.map(|r| 1.0 / r as f64).unwrap_or(0.0))
            .sum::<f64>()
            / total as f64
    };
    let failures = outcomes.iter().filter(|o| !o.passed()).cloned().collect();
    EvalSummary {
        total,
        passed,
        mrr,
        failures,
    }
}
