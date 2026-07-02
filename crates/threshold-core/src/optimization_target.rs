//! Crucible-backed optimization target import and headroom probe.
//!
//! This is the first narrow slice of backlog 061: pull a Crucible
//! `crucible.eval_spec.v1` key-recall eval into Threshold, summarize the
//! existing freeze evidence as a bounded headroom probe, and leave a
//! Bitterblossom/Sprites trial request + receipt surface for the remote runner
//! seam. It intentionally does not mutate graders, arenas, or Bitterblossom
//! plane files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const CRUCIBLE_EVAL_SCHEMA: &str = "crucible.eval_spec.v1";
const TARGET_SCHEMA: &str = "threshold.optimization_target.v1";
const HEADROOM_SCHEMA: &str = "threshold.headroom_probe.v1";
const GUARDRAILS_SCHEMA: &str = "threshold.optimizer_guardrails.v1";
const SPRITE_REQUEST_SCHEMA: &str = "threshold.sprite_trial_request.v1";
const SPRITE_RECEIPT_SCHEMA: &str = "threshold.sprite_trial_receipt.v1";
const OPTIMIZER_LOOP_SCHEMA: &str = "threshold.optimizer_loop.v1";
const ASHA_SCHEMA: &str = "threshold.asha.v1";
const PARETO_SCHEMA: &str = "threshold.pareto_frontier.v1";
const CERTIFICATION_SCHEMA: &str = "threshold.heldout_certification.v1";
const SPRITE_TRIAL_MAX_COST_USD: f64 = 0.60;

#[derive(Debug, Clone)]
pub struct OptimizationTargetError(pub String);

impl std::fmt::Display for OptimizationTargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OptimizationTargetError {}

#[derive(Debug, Clone)]
pub struct HeadroomProbeOptions {
    pub eval_spec: PathBuf,
    pub out_dir: PathBuf,
    pub budget_usd: f64,
    pub bb_config: Option<PathBuf>,
    pub bb_task: String,
    pub bb_bin: PathBuf,
    pub bb_repo: String,
    pub bb_rev: Option<String>,
    pub bb_change: Option<String>,
    pub bb_submission: Option<String>,
    pub dispatch_bitterblossom: bool,
}

#[derive(Debug, Clone)]
pub struct HeadroomProbeResult {
    pub out_dir: PathBuf,
    pub target: PathBuf,
    pub rig: PathBuf,
    pub headroom_probe: PathBuf,
    pub guardrails: PathBuf,
    pub trials: PathBuf,
    pub report: PathBuf,
    pub sprite_request: PathBuf,
    pub sprite_receipt: PathBuf,
    pub verdict: String,
    pub probe_point: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct OptimizerLoopOptions {
    pub eval_spec: PathBuf,
    pub out_dir: PathBuf,
    pub budget_usd: f64,
    pub bb_config: Option<PathBuf>,
    pub bb_tasks: Vec<String>,
    pub bb_bin: PathBuf,
    pub bb_repo: String,
    pub bb_rev: Option<String>,
    pub bb_change: Option<String>,
    pub dispatch_bitterblossom: bool,
    pub certify_top: usize,
    pub rng_seed: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct OptimizerLoopResult {
    pub out_dir: PathBuf,
    pub target: PathBuf,
    pub rig: PathBuf,
    pub seed: PathBuf,
    pub headroom_probe: PathBuf,
    pub guardrails: PathBuf,
    pub asha: PathBuf,
    pub pareto: PathBuf,
    pub certification: PathBuf,
    pub history: PathBuf,
    pub report: PathBuf,
    pub candidates: usize,
    pub frontier: usize,
    pub spend_known_usd: Option<f64>,
}

#[derive(Debug, Clone)]
struct EvalInfo {
    spec: Value,
    id: String,
    decision: Option<String>,
    baselines: Vec<String>,
    runner_kind: String,
    source_trials: PathBuf,
    arena_dir: Option<PathBuf>,
    candidate_id: String,
    tasks: Vec<String>,
    eval_digest: String,
    trials_digest: String,
}

#[derive(Debug, Clone)]
struct CandidateSummary {
    candidate_id: String,
    candidate_kind: String,
    model: Option<String>,
    composition_hash: String,
    successes: u64,
    n: u64,
    point: Option<f64>,
    lower: Option<f64>,
    upper: Option<f64>,
    cost_usd_total: Option<f64>,
    wall_ms_total: u64,
    trials: u64,
    expected_defects: u64,
    false_positives: u64,
    reward_mean: Option<f64>,
    task_results: Vec<Value>,
}

pub fn run_headroom_probe(
    options: &HeadroomProbeOptions,
) -> Result<HeadroomProbeResult, Box<dyn std::error::Error>> {
    if !(options.budget_usd.is_finite() && options.budget_usd > 0.0) {
        return Err(OptimizationTargetError("--budget-usd must be positive".to_string()).into());
    }
    if options.bb_task.trim().is_empty() {
        return Err(OptimizationTargetError("--bb-task must not be empty".to_string()).into());
    }
    if options.dispatch_bitterblossom {
        if options.bb_repo.trim().is_empty() {
            return Err(OptimizationTargetError(
                "--bb-repo must not be empty when dispatching".to_string(),
            )
            .into());
        }
        if options.bb_rev.as_deref().unwrap_or("").trim().is_empty() {
            return Err(OptimizationTargetError(
                "--bb-rev must resolve to a fetchable revision when dispatching".to_string(),
            )
            .into());
        }
    }

    let eval_info = load_eval_info(&options.eval_spec)?;
    let all_records = read_jsonl_values(&eval_info.source_trials)?;
    let candidates = candidate_ids(&eval_info);
    let summaries = summarize_candidates(&eval_info, &all_records, &candidates)?;
    let selected_records = selected_trial_records(&eval_info, &all_records, &candidates);

    std::fs::create_dir_all(&options.out_dir)?;
    let target_path = options.out_dir.join("optimization-target.json");
    let rig_path = options.out_dir.join("rig.json");
    let headroom_path = options.out_dir.join("headroom-probe.json");
    let guardrails_path = options.out_dir.join("guardrails.json");
    let trials_path = options.out_dir.join("trials.jsonl");
    let report_path = options.out_dir.join("report.md");
    let request_path = options.out_dir.join("sprite-trial-request.json");
    let receipt_path = options.out_dir.join("sprite-trial-receipt.json");

    let target = build_target(&eval_info, options);
    let rig = build_rig(&eval_info, options, &candidates);
    let headroom = build_headroom_probe(&eval_info, options, &summaries);
    let guardrails = build_guardrails(&eval_info, &headroom);
    let request = build_sprite_request(&eval_info, options, &summaries, &target);

    write_json(&target_path, &target)?;
    write_json(&rig_path, &rig)?;
    write_json(&headroom_path, &headroom)?;
    write_json(&guardrails_path, &guardrails)?;
    write_trials_jsonl(&trials_path, &selected_records, &eval_info)?;
    write_json(&request_path, &request)?;

    let receipt = if options.dispatch_bitterblossom {
        dispatch_bitterblossom(options, &request_path, &request)
    } else {
        build_pending_receipt(&request)
    };
    write_json(&receipt_path, &receipt)?;

    let report = render_report(&eval_info, options, &summaries, &headroom, &receipt);
    std::fs::write(&report_path, report)?;

    let verdict = headroom
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let probe_point = summaries
        .iter()
        .find(|s| s.candidate_id == eval_info.candidate_id)
        .and_then(|s| s.point);

    Ok(HeadroomProbeResult {
        out_dir: options.out_dir.clone(),
        target: target_path,
        rig: rig_path,
        headroom_probe: headroom_path,
        guardrails: guardrails_path,
        trials: trials_path,
        report: report_path,
        sprite_request: request_path,
        sprite_receipt: receipt_path,
        verdict,
        probe_point,
    })
}

pub fn run_optimizer_loop(
    options: &OptimizerLoopOptions,
) -> Result<OptimizerLoopResult, Box<dyn std::error::Error>> {
    if !(options.budget_usd.is_finite() && options.budget_usd > 0.0) {
        return Err(OptimizationTargetError("--budget-usd must be positive".to_string()).into());
    }
    if options.bb_tasks.is_empty() {
        return Err(OptimizationTargetError(
            "--bb-task must be supplied at least once".to_string(),
        )
        .into());
    }
    if options.bb_tasks.iter().any(|task| task.trim().is_empty()) {
        return Err(
            OptimizationTargetError("--bb-task entries must not be empty".to_string()).into(),
        );
    }
    if options.dispatch_bitterblossom {
        if options.bb_repo.trim().is_empty() {
            return Err(OptimizationTargetError(
                "--bb-repo must not be empty when dispatching".to_string(),
            )
            .into());
        }
        if options.bb_rev.as_deref().unwrap_or("").trim().is_empty() {
            return Err(OptimizationTargetError(
                "--bb-rev must resolve to a fetchable revision when dispatching".to_string(),
            )
            .into());
        }
    }

    let eval_info = load_eval_info(&options.eval_spec)?;
    let all_records = read_jsonl_values(&eval_info.source_trials)?;
    let reference_candidates = candidate_ids(&eval_info);
    let summaries = summarize_candidates(&eval_info, &all_records, &reference_candidates)?;
    let selected_records = selected_trial_records(&eval_info, &all_records, &reference_candidates);
    let splits = split_tasks(&eval_info.tasks);
    let first_task = options
        .bb_tasks
        .first()
        .cloned()
        .unwrap_or_else(|| "correctness".to_string());
    let headroom_options = HeadroomProbeOptions {
        eval_spec: options.eval_spec.clone(),
        out_dir: options.out_dir.clone(),
        budget_usd: options.budget_usd,
        bb_config: options.bb_config.clone(),
        bb_task: first_task.clone(),
        bb_bin: options.bb_bin.clone(),
        bb_repo: options.bb_repo.clone(),
        bb_rev: options.bb_rev.clone(),
        bb_change: options.bb_change.clone(),
        bb_submission: None,
        dispatch_bitterblossom: false,
    };
    let target = build_target(&eval_info, &headroom_options);
    let rig = build_optimizer_rig(&eval_info, options, &reference_candidates, &splits);
    let headroom = build_headroom_probe(&eval_info, &headroom_options, &summaries);
    if headroom.get("verdict").and_then(Value::as_str) != Some("pass") {
        return Err(OptimizationTargetError(format!(
            "headroom probe did not pass; verdict={}",
            headroom
                .get("verdict")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ))
        .into());
    }
    let incumbent = summaries
        .iter()
        .find(|s| s.candidate_id == eval_info.candidate_id)
        .ok_or_else(|| {
            OptimizationTargetError("incumbent candidate summary missing".to_string())
        })?;
    let optimizer_candidates = build_optimizer_candidates(&eval_info, options, incumbent);

    std::fs::create_dir_all(&options.out_dir)?;
    let request_dir = options.out_dir.join("sprite-requests");
    let receipt_dir = options.out_dir.join("sprite-receipts");
    let bb_result_dir = options.out_dir.join("bitterblossom-results");
    std::fs::create_dir_all(&request_dir)?;
    std::fs::create_dir_all(&receipt_dir)?;
    std::fs::create_dir_all(&bb_result_dir)?;

    let target_path = options.out_dir.join("optimization-target.json");
    let rig_path = options.out_dir.join("rig.json");
    let seed_path = options.out_dir.join("seed.json");
    let headroom_path = options.out_dir.join("headroom-probe.json");
    let guardrails_path = options.out_dir.join("guardrails.json");
    let trials_path = options.out_dir.join("trials.jsonl");
    let history_path = options.out_dir.join("loop.history.jsonl");
    let asha_path = options.out_dir.join("asha.json");
    let pareto_path = options.out_dir.join("pareto.json");
    let certification_path = options.out_dir.join("certification.json");
    let report_path = options.out_dir.join("report.md");

    write_json(&target_path, &target)?;
    write_json(&rig_path, &rig)?;
    write_json(
        &seed_path,
        &build_seed(&eval_info, options, &optimizer_candidates, &splits),
    )?;
    write_json(&headroom_path, &headroom)?;
    write_trials_jsonl(&trials_path, &selected_records, &eval_info)?;

    let mut history = Vec::new();
    let mut validation_trials = Vec::new();
    let mut receipts = Vec::new();
    let mut authorized_spend_usd = 0.0_f64;
    for candidate in &optimizer_candidates {
        reserve_optimizer_dispatch_budget(
            &mut authorized_spend_usd,
            options,
            candidate,
            "validation",
        )?;
        let trial = run_sprite_optimizer_trial(
            &eval_info,
            options,
            candidate,
            "validation",
            &splits.validation,
            incumbent,
            &target,
            &request_dir,
            &receipt_dir,
            &bb_result_dir,
        )?;
        receipts.push(trial.receipt.clone());
        history.push(history_entry(0, "validation", &trial));
        validation_trials.push(trial.to_value());
    }

    let frontier = pareto_frontier(&validation_trials);
    let mut promoted = frontier.clone();
    promoted.sort_by(compare_candidates_for_promotion);
    promoted.truncate(options.certify_top.max(1).min(promoted.len()));

    let mut certification_trials = Vec::new();
    for promoted_candidate in &promoted {
        let candidate_id = promoted_candidate
            .get("candidate_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(candidate) = optimizer_candidates.iter().find(|c| c.id == candidate_id) else {
            continue;
        };
        reserve_optimizer_dispatch_budget(
            &mut authorized_spend_usd,
            options,
            candidate,
            "heldout",
        )?;
        let trial = run_sprite_optimizer_trial(
            &eval_info,
            options,
            candidate,
            "heldout",
            &splits.heldout,
            incumbent,
            &target,
            &request_dir,
            &receipt_dir,
            &bb_result_dir,
        )?;
        receipts.push(trial.receipt.clone());
        history.push(history_entry(1, "heldout", &trial));
        certification_trials.push(trial.to_value());
    }

    let asha = build_asha(
        &eval_info,
        options,
        &validation_trials,
        &promoted,
        &certification_trials,
    );
    let pareto = build_pareto(&eval_info, &validation_trials, &frontier);
    let certification = build_certification(&eval_info, &splits, &promoted, &certification_trials);
    let guardrails = build_optimizer_guardrails(&eval_info, &headroom, &splits, &receipts);

    write_json(&guardrails_path, &guardrails)?;
    write_json(&asha_path, &asha)?;
    write_json(&pareto_path, &pareto)?;
    write_json(&certification_path, &certification)?;
    write_history(&history_path, &history)?;
    std::fs::write(
        &report_path,
        render_optimizer_report(
            &eval_info,
            options,
            &splits,
            &validation_trials,
            &frontier,
            &certification_trials,
            &headroom,
        ),
    )?;

    Ok(OptimizerLoopResult {
        out_dir: options.out_dir.clone(),
        target: target_path,
        rig: rig_path,
        seed: seed_path,
        headroom_probe: headroom_path,
        guardrails: guardrails_path,
        asha: asha_path,
        pareto: pareto_path,
        certification: certification_path,
        history: history_path,
        report: report_path,
        candidates: optimizer_candidates.len(),
        frontier: frontier.len(),
        spend_known_usd: known_receipt_spend(&receipts),
    })
}

fn reserve_optimizer_dispatch_budget(
    authorized_spend_usd: &mut f64,
    options: &OptimizerLoopOptions,
    candidate: &OptimizerCandidate,
    split_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !options.dispatch_bitterblossom {
        return Ok(());
    }
    let next_authorized = *authorized_spend_usd + SPRITE_TRIAL_MAX_COST_USD;
    if next_authorized > options.budget_usd {
        return Err(OptimizationTargetError(format!(
            "optimizer budget cap would be exceeded before dispatching {} {split_name}: authorized ${:.2} + trial max ${:.2} > cap ${:.2}",
            candidate.id, *authorized_spend_usd, SPRITE_TRIAL_MAX_COST_USD, options.budget_usd
        ))
        .into());
    }
    *authorized_spend_usd = next_authorized;
    Ok(())
}

fn load_eval_info(eval_spec: &Path) -> Result<EvalInfo, Box<dyn std::error::Error>> {
    let spec_text = std::fs::read_to_string(eval_spec).map_err(|err| {
        OptimizationTargetError(format!("read Crucible eval {}: {err}", eval_spec.display()))
    })?;
    let spec: Value = serde_json::from_str(&spec_text).map_err(|err| {
        OptimizationTargetError(format!(
            "parse Crucible eval {}: {err}",
            eval_spec.display()
        ))
    })?;
    if spec.get("schema_version").and_then(Value::as_str) != Some(CRUCIBLE_EVAL_SCHEMA) {
        return Err(OptimizationTargetError(format!(
            "{} is not {CRUCIBLE_EVAL_SCHEMA}",
            eval_spec.display()
        ))
        .into());
    }
    let id = required_str(&spec, &["id"], "eval id")?.to_string();
    let runner = spec
        .get("runner")
        .and_then(Value::as_object)
        .ok_or_else(|| OptimizationTargetError("runner is required".to_string()))?;
    let runner_kind = runner
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| OptimizationTargetError("runner.kind is required".to_string()))?
        .to_string();
    if runner_kind != "key_recall" {
        return Err(OptimizationTargetError(format!(
            "unsupported Crucible runner.kind {runner_kind:?}; only key_recall is supported"
        ))
        .into());
    }
    let corpus = runner
        .get("corpus")
        .and_then(Value::as_object)
        .ok_or_else(|| OptimizationTargetError("runner.corpus is required".to_string()))?;
    let base = eval_spec.parent().unwrap_or_else(|| Path::new("."));
    let source_trials = resolve_existing(base, required_str_in(corpus, "trials_jsonl")?)?;
    let arena_dir = corpus
        .get("arena_dir")
        .and_then(Value::as_str)
        .map(|p| resolve_existing(base, p))
        .transpose()?;
    let candidate_id = required_str_in(corpus, "candidate_id")?.to_string();
    let tasks = corpus
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| OptimizationTargetError("runner.corpus.tasks is required".to_string()))?
        .iter()
        .map(|v| {
            v.as_str().map(str::to_string).ok_or_else(|| {
                OptimizationTargetError("runner.corpus.tasks entries must be strings".to_string())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if tasks.is_empty() {
        return Err(
            OptimizationTargetError("runner.corpus.tasks must not be empty".to_string()).into(),
        );
    }
    let baselines = spec
        .get("baselines")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec!["null".to_string(), "oracle".to_string()]);
    let eval_digest = format!("sha256:{}", sha256_hex(spec_text.as_bytes()));
    let trials_bytes = std::fs::read(&source_trials)?;
    let trials_digest = format!("sha256:{}", sha256_hex(&trials_bytes));
    let decision = spec
        .get("decision")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(EvalInfo {
        spec,
        id,
        decision,
        baselines,
        runner_kind,
        source_trials,
        arena_dir,
        candidate_id,
        tasks,
        eval_digest,
        trials_digest,
    })
}

fn summarize_candidates(
    eval: &EvalInfo,
    records: &[Value],
    candidates: &[String],
) -> Result<Vec<CandidateSummary>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for candidate_id in candidates {
        let mut by_task: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for record in records {
            let rec_candidate = record.get("candidate_id").and_then(Value::as_str);
            let task = record.get("task_id").and_then(Value::as_str);
            if rec_candidate == Some(candidate_id.as_str()) {
                if let Some(task) = task {
                    if eval.tasks.iter().any(|t| t == task) {
                        by_task
                            .entry(task.to_string())
                            .or_default()
                            .push(record.clone());
                    }
                }
            }
        }
        let missing: Vec<String> = eval
            .tasks
            .iter()
            .filter(|task| !by_task.contains_key(task.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Err(OptimizationTargetError(format!(
                "source trials missing candidate {candidate_id} for task(s): {}",
                missing.join(", ")
            ))
            .into());
        }
        let mut successes = 0_u64;
        let mut n = 0_u64;
        let mut known_cost = true;
        let mut cost_total = 0.0_f64;
        let mut wall_total = 0_u64;
        let mut false_positives = 0_u64;
        let mut reward_values = Vec::new();
        let mut task_results = Vec::new();
        let mut trial_count = 0_u64;
        let mut candidate_kind = String::new();
        let mut model = None;
        let mut composition_hash = String::new();

        for task in &eval.tasks {
            let task_records = by_task.get(task).expect("checked above");
            let mut task_successes = 0_u64;
            let mut task_n = 0_u64;
            let mut task_false_positives = 0_u64;
            let mut task_rewards = Vec::new();
            let mut source_run_ids = Vec::new();
            for record in task_records {
                let record_kind = record
                    .get("candidate_kind")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if candidate_kind.is_empty() {
                    candidate_kind = record_kind.to_string();
                }
                if model.is_none() {
                    model = record
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                if composition_hash.is_empty() {
                    composition_hash = record
                        .get("composition_hash")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                let expected = record
                    .get("expected_defects")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let matched = record
                    .get("matched")
                    .and_then(Value::as_array)
                    .map(|a| a.len() as u64)
                    .unwrap_or(0)
                    .min(expected);
                let fps = record
                    .get("false_positives")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let reward = record.get("reward").and_then(Value::as_f64);
                successes += matched;
                n += expected;
                task_successes += matched;
                task_n += expected;
                false_positives += fps;
                task_false_positives += fps;
                if let Some(reward) = reward {
                    reward_values.push(reward);
                    task_rewards.push(reward);
                }
                match record.get("cost_usd") {
                    Some(Value::Number(num)) => {
                        cost_total += num.as_f64().unwrap_or(0.0);
                    }
                    Some(Value::Null) | None if is_costless_kind(record_kind) => {}
                    _ => known_cost = false,
                }
                wall_total += record.get("wall_ms").and_then(Value::as_u64).unwrap_or(0);
                trial_count += 1;
                source_run_ids.push(record.get("run_id").cloned().unwrap_or(Value::Null));
            }
            let task_reward_mean = if task_rewards.is_empty() {
                None
            } else {
                Some(task_rewards.iter().sum::<f64>() / task_rewards.len() as f64)
            };
            task_results.push(json!({
                "task_id": task,
                "successes": task_successes,
                "n": task_n,
                "recall": if task_n > 0 { Some(task_successes as f64 / task_n as f64) } else { None },
                "reward_mean": task_reward_mean,
                "false_positives": task_false_positives,
                "trials": task_records.len(),
                "source_run_ids": source_run_ids
            }));
        }
        let point = if n > 0 {
            Some(successes as f64 / n as f64)
        } else {
            None
        };
        let (lower, upper) = match (successes, n) {
            (_, 0) => (None, None),
            (s, n) => {
                let (lo, hi) = wilson_interval(s, n);
                (Some(lo), Some(hi))
            }
        };
        let reward_mean = if reward_values.is_empty() {
            None
        } else {
            Some(reward_values.iter().sum::<f64>() / reward_values.len() as f64)
        };
        out.push(CandidateSummary {
            candidate_id: candidate_id.clone(),
            candidate_kind,
            model,
            composition_hash,
            successes,
            n,
            point,
            lower,
            upper,
            cost_usd_total: known_cost.then_some(cost_total),
            wall_ms_total: wall_total,
            trials: trial_count,
            expected_defects: n,
            false_positives,
            reward_mean,
            task_results,
        });
    }
    Ok(out)
}

fn selected_trial_records(eval: &EvalInfo, records: &[Value], candidates: &[String]) -> Vec<Value> {
    let candidate_set: BTreeSet<&str> = candidates.iter().map(String::as_str).collect();
    let task_set: BTreeSet<&str> = eval.tasks.iter().map(String::as_str).collect();
    records
        .iter()
        .filter(|record| {
            let candidate = record.get("candidate_id").and_then(Value::as_str);
            let task = record.get("task_id").and_then(Value::as_str);
            candidate.is_some_and(|c| candidate_set.contains(c))
                && task.is_some_and(|t| task_set.contains(t))
        })
        .cloned()
        .collect()
}

fn build_target(eval: &EvalInfo, options: &HeadroomProbeOptions) -> Value {
    json!({
        "schema": TARGET_SCHEMA,
        "id": eval.id,
        "source": "crucible",
        "mode": "threshold-then-cheap",
        "decision": eval.decision,
        "eval": {
            "schema": CRUCIBLE_EVAL_SCHEMA,
            "spec_ref": eval.spec.get("id").and_then(Value::as_str).unwrap_or(""),
            "spec_path": path_string(&options.eval_spec),
            "digest": eval.eval_digest,
            "harbor_package": eval.arena_dir.as_ref().map(|p| path_string(p)),
            "fixtures_digest": Value::Null,
            "answer_key_digest": Value::Null,
            "oracle_solution_digest": Value::Null,
            "source_trials_jsonl": path_string(&eval.source_trials),
            "source_trials_digest": eval.trials_digest,
            "version": eval.spec.get("version").cloned().unwrap_or(Value::Null)
        },
        "scorer": {
            "kind": "threshold-score",
            "metric": "pr_review_key_recall",
            "authoritative_until": "crucible-backlog-008-grade-reward-parity",
            "note": "This first slice imports Crucible key-recall evidence from a frozen Threshold run; full Harbor bundle digests remain a guardrail gap."
        },
        "splits": {
            "train": Value::Null,
            "validation": "runner.corpus.tasks",
            "holdout": Value::Null,
            "holdout_policy": "not declared by this Crucible key-recall eval"
        },
        "baselines": eval.baselines,
        "runner": {
            "default": eval.runner_kind,
            "remote": "bitter-blossom-sprites",
            "request_schema": SPRITE_REQUEST_SCHEMA,
            "receipt_schema": SPRITE_RECEIPT_SCHEMA,
            "bb_task": options.bb_task
        },
        "gates": {
            "g1": {"state": "unsigned", "meaning": "operator approval required before paid search beyond this bounded probe"},
            "g2": {"state": "unsigned", "meaning": "eval quality/headroom not fully approved; this probe is evidence for that decision"},
            "g3": {"state": "unsigned", "meaning": "launch approval required before downstream deployment"},
            "g5": {"state": "not_applicable", "meaning": "authored Crucible eval, not production trace re-ingestion"}
        }
    })
}

fn build_rig(eval: &EvalInfo, options: &HeadroomProbeOptions, candidates: &[String]) -> Value {
    json!({
        "schema_version": "threshold.optimization_rig.v1",
        "eval_id": eval.id,
        "eval_digest": eval.eval_digest,
        "source_trials": path_string(&eval.source_trials),
        "source_trials_digest": eval.trials_digest,
        "arena_dir": eval.arena_dir.as_ref().map(|p| path_string(p)),
        "runner_kind": eval.runner_kind,
        "tasks": eval.tasks,
        "candidates": candidates,
        "headroom_budget_usd": options.budget_usd,
        "remote_runner": {
            "plane": "bitterblossom",
            "substrate": "sprites",
            "config": options.bb_config.as_ref().map(|p| path_string(p)),
            "task": options.bb_task,
            "repo": options.bb_repo,
            "rev": options.bb_rev,
            "change": options.bb_change,
            "submission": options.bb_submission,
            "dispatch_requested": options.dispatch_bitterblossom
        }
    })
}

fn build_headroom_probe(
    eval: &EvalInfo,
    options: &HeadroomProbeOptions,
    summaries: &[CandidateSummary],
) -> Value {
    let oracle = summaries.iter().find(|s| s.candidate_id == "oracle");
    let null = summaries.iter().find(|s| s.candidate_id == "null");
    let probe = summaries
        .iter()
        .find(|s| s.candidate_id == eval.candidate_id);
    let oracle_point = oracle.and_then(|s| s.point);
    let null_point = null.and_then(|s| s.point);
    let probe_point = probe.and_then(|s| s.point);
    let oracle_pass = oracle_point.is_some_and(|p| (p - 1.0).abs() < 1e-9);
    let null_pass = null_point.is_some_and(|p| p <= 0.000_001);
    let saturated =
        oracle_pass && matches!((oracle_point, probe_point), (Some(o), Some(p)) if p >= o - 0.1);
    let ranks = oracle_point.is_some()
        && null_point.is_some()
        && probe_point.is_some()
        && !saturated
        && oracle_point.unwrap_or(0.0) > null_point.unwrap_or(0.0);
    let verdict = if oracle_pass && null_pass && ranks {
        "pass"
    } else if saturated {
        "saturated"
    } else {
        "needs-review"
    };
    let candidate_values: Vec<Value> = summaries.iter().map(candidate_to_value).collect();
    let known_spend = summaries
        .iter()
        .try_fold(0.0_f64, |acc, c| c.cost_usd_total.map(|v| acc + v));
    json!({
        "schema": HEADROOM_SCHEMA,
        "eval_id": eval.id,
        "budget_usd": options.budget_usd,
        "spend_known_usd": known_spend,
        "metric": "defect_level_key_recall",
        "uncertainty": {
            "method": "wilson",
            "confidence": 0.95
        },
        "verdict": verdict,
        "checks": {
            "oracle_reaches_ceiling": oracle_pass,
            "null_is_floor": null_pass,
            "oneshot_not_saturated": !saturated,
            "reference_spread_exists": ranks,
            "under_budget": known_spend.is_some_and(|s| s <= options.budget_usd)
        },
        "candidates": candidate_values
    })
}

fn build_guardrails(eval: &EvalInfo, headroom: &Value) -> Value {
    json!({
        "schema": GUARDRAILS_SCHEMA,
        "eval_id": eval.id,
        "overfitting": {
            "status": "partial",
            "evidence": "imported key-recall probe uses the Crucible-declared task subset; no GEPA mutation or holdout reuse occurs in this slice",
            "holdout_feedback_blocked": true
        },
        "judge_gaming": {
            "status": "pass",
            "evidence": "objective is deterministic key recall from Threshold trial records; no judge-only winner is selected"
        },
        "non_stationarity": {
            "status": "pass",
            "eval_digest": eval.eval_digest,
            "source_trials_digest": eval.trials_digest,
            "model_provider_ids_recorded": true
        },
        "scorer_parity": {
            "status": "caveat",
            "evidence": "Crucible eval points at Threshold freeze evidence; Crucible grade parity remains outside this slice"
        },
        "harbor_bundle": {
            "status": if eval.arena_dir.is_some() { "partial" } else { "missing" },
            "evidence": "Crucible key-recall eval declares source trials and arena_dir, but not full answer-key/scorer/oracle digests"
        },
        "headroom_verdict": headroom.get("verdict").cloned().unwrap_or(Value::Null)
    })
}

#[derive(Debug, Clone)]
struct TaskSplits {
    validation: Vec<String>,
    heldout: Vec<String>,
}

#[derive(Debug, Clone)]
struct OptimizerCandidate {
    id: String,
    parent_id: String,
    bb_task: String,
    model: String,
    mutation: String,
    hypothesis: String,
    composition_hash: String,
    prompt_packet_digest: String,
}

#[derive(Debug, Clone)]
struct SubmissionRef {
    id: String,
    change_key: String,
    row: Value,
}

#[derive(Debug, Clone)]
struct OptimizerTrial {
    candidate_id: String,
    task_id: String,
    split: String,
    score: f64,
    source_key_recall: Option<f64>,
    remote_verdict_score: f64,
    cost_usd: Option<f64>,
    wall_ms: Option<u64>,
    request_path: PathBuf,
    receipt_path: PathBuf,
    result_path: Option<PathBuf>,
    receipt: Value,
}

impl OptimizerTrial {
    fn to_value(&self) -> Value {
        json!({
            "candidate_id": self.candidate_id,
            "task_id": self.task_id,
            "split": self.split,
            "score": self.score,
            "score_source": {
                "formula": "source_split_key_recall * remote_verdict_score",
                "source_key_recall": self.source_key_recall,
                "remote_verdict_score": self.remote_verdict_score,
                "note": "Until Crucible grade parity lands, Threshold keeps deterministic key-recall primary and treats the BB/Sprites verdict as a remote execution quality gate."
            },
            "cost_usd": self.cost_usd,
            "wall_ms": self.wall_ms,
            "sprite_request": path_string(&self.request_path),
            "sprite_receipt": path_string(&self.receipt_path),
            "bitterblossom_result": self.result_path.as_ref().map(|p| path_string(p)),
            "bitter_blossom_run_id": self.receipt.get("bitter_blossom_run_id").cloned().unwrap_or(Value::Null),
            "status": self.receipt.get("status").cloned().unwrap_or(Value::Null)
        })
    }
}

fn split_tasks(tasks: &[String]) -> TaskSplits {
    if tasks.len() <= 1 {
        return TaskSplits {
            validation: tasks.to_vec(),
            heldout: tasks.to_vec(),
        };
    }
    let heldout_len = (tasks.len() / 3).max(1);
    let split_at = tasks.len().saturating_sub(heldout_len);
    TaskSplits {
        validation: tasks[..split_at].to_vec(),
        heldout: tasks[split_at..].to_vec(),
    }
}

fn build_optimizer_candidates(
    eval: &EvalInfo,
    options: &OptimizerLoopOptions,
    incumbent: &CandidateSummary,
) -> Vec<OptimizerCandidate> {
    let templates = [
        (
            "gepa-evidence-first",
            "Use failure evidence to require precise file, line, and invariant support before emitting a defect.",
        ),
        (
            "gepa-false-positive-averse",
            "Penalize speculative findings and prefer fewer findings that match keyed defects over broad audit commentary.",
        ),
        (
            "gepa-caller-context",
            "Force a caller/invariant retrieval pass before deciding whether a diff-local issue is real.",
        ),
        (
            "gepa-clean-fixture-sentinel",
            "Carry a clean-fixture check so empty or rename-only diffs can return no findings without apology.",
        ),
    ];
    options
        .bb_tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| {
            let seed_offset = options
                .rng_seed
                .map(|seed| seed as usize % templates.len())
                .unwrap_or(0);
            let (mutation, hypothesis) = templates[(idx + seed_offset) % templates.len()];
            let digest_basis = format!(
                "{}:{}:{}:{}",
                eval.id, incumbent.composition_hash, task, mutation
            );
            let hash = sha256_hex(digest_basis.as_bytes());
            OptimizerCandidate {
                id: safe_id(&format!("{mutation}-{task}")),
                parent_id: incumbent.candidate_id.clone(),
                bb_task: task.clone(),
                model: model_for_bb_task(task),
                mutation: mutation.to_string(),
                hypothesis: hypothesis.to_string(),
                composition_hash: hash.clone(),
                prompt_packet_digest: format!("sha256:{}", sha256_hex(hypothesis.as_bytes())),
            }
        })
        .collect()
}

fn build_optimizer_rig(
    eval: &EvalInfo,
    options: &OptimizerLoopOptions,
    reference_candidates: &[String],
    splits: &TaskSplits,
) -> Value {
    json!({
        "schema_version": "threshold.optimization_rig.v1",
        "loop_schema": OPTIMIZER_LOOP_SCHEMA,
        "eval_id": eval.id,
        "eval_digest": eval.eval_digest,
        "source_trials": path_string(&eval.source_trials),
        "source_trials_digest": eval.trials_digest,
        "arena_dir": eval.arena_dir.as_ref().map(|p| path_string(p)),
        "runner_kind": eval.runner_kind,
        "reference_candidates": reference_candidates,
        "tasks": eval.tasks,
        "splits": {
            "validation": splits.validation,
            "heldout": splits.heldout,
            "policy": "deterministic tail holdout until Crucible export declares explicit train/validation/holdout ids"
        },
        "budget": {
            "cap_usd": options.budget_usd,
            "allocator": "hyperband-asha",
            "rungs": ["validation:1x", "heldout:promoted"]
        },
        "gepa": {
            "inner_loop": "evidence-driven reflective prompt mutation",
            "archive": "score,cost Pareto frontier",
            "mutation_slots": ["packet_stance", "false_positive_policy", "retrieval_stance"]
        },
        "remote_runner": {
            "plane": "bitterblossom",
            "substrate": "sprites",
            "config": options.bb_config.as_ref().map(|p| path_string(p)),
            "tasks": options.bb_tasks,
            "repo": options.bb_repo,
            "rev": options.bb_rev,
            "change": options.bb_change,
            "dispatch_requested": options.dispatch_bitterblossom
        }
    })
}

fn build_seed(
    eval: &EvalInfo,
    options: &OptimizerLoopOptions,
    candidates: &[OptimizerCandidate],
    splits: &TaskSplits,
) -> Value {
    json!({
        "schema": "threshold.optimizer_seed.v1",
        "eval_id": eval.id,
        "eval_digest": eval.eval_digest,
        "source_trials_digest": eval.trials_digest,
        "budget_usd": options.budget_usd,
        "rng_seed": options.rng_seed,
        "allocator": {
            "kind": "hyperband-asha",
            "eta": 3,
            "rung0": {
                "name": "validation",
                "budget_units": 1,
                "candidates": candidates.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
            },
            "rung1": {
                "name": "heldout",
                "budget_units": 1,
                "promote": options.certify_top.max(1)
            }
        },
        "splits": {
            "validation": splits.validation,
            "heldout": splits.heldout
        },
        "gepa_candidates": candidates.iter().map(|c| json!({
            "candidate_id": c.id,
            "parent_id": c.parent_id,
            "bb_task": c.bb_task,
            "model": c.model,
            "mutation": c.mutation,
            "hypothesis": c.hypothesis,
            "composition_hash": format_hash(&c.composition_hash),
            "prompt_packet_digest": c.prompt_packet_digest
        })).collect::<Vec<_>>()
    })
}

#[allow(clippy::too_many_arguments)]
fn run_sprite_optimizer_trial(
    eval: &EvalInfo,
    options: &OptimizerLoopOptions,
    candidate: &OptimizerCandidate,
    split_name: &str,
    split_tasks: &[String],
    incumbent: &CandidateSummary,
    target: &Value,
    request_dir: &Path,
    receipt_dir: &Path,
    bb_result_dir: &Path,
) -> Result<OptimizerTrial, Box<dyn std::error::Error>> {
    let submission = if options.dispatch_bitterblossom {
        Some(open_bitterblossom_submission(
            options, candidate, split_name,
        )?)
    } else {
        None
    };
    let request = build_optimizer_sprite_request(
        eval,
        options,
        candidate,
        split_name,
        split_tasks,
        target,
        submission.as_ref(),
    );
    let file_stem = format!("{}-{}", candidate.id, split_name);
    let request_path = request_dir.join(format!("{file_stem}.json"));
    let receipt_path = receipt_dir.join(format!("{file_stem}.json"));
    write_json(&request_path, &request)?;

    let headroom_options = HeadroomProbeOptions {
        eval_spec: options.eval_spec.clone(),
        out_dir: options.out_dir.clone(),
        budget_usd: options.budget_usd,
        bb_config: options.bb_config.clone(),
        bb_task: candidate.bb_task.clone(),
        bb_bin: options.bb_bin.clone(),
        bb_repo: options.bb_repo.clone(),
        bb_rev: options.bb_rev.clone(),
        bb_change: options.bb_change.clone(),
        bb_submission: submission.as_ref().map(|s| s.id.clone()),
        dispatch_bitterblossom: options.dispatch_bitterblossom,
    };
    let receipt = if options.dispatch_bitterblossom {
        dispatch_bitterblossom(&headroom_options, &request_path, &request)
    } else {
        build_pending_receipt(&request)
    };
    write_json(&receipt_path, &receipt)?;

    let artifact = read_bitterblossom_result(options, &receipt);
    let result_path = if !artifact.is_null() {
        let result_path = bb_result_dir.join(format!("{file_stem}.json"));
        write_json(&result_path, &artifact)?;
        Some(result_path)
    } else {
        None
    };
    let split_score = split_key_recall(incumbent, split_tasks);
    let remote_score = remote_verdict_score(&receipt, &artifact);
    let score = split_score.unwrap_or(0.0) * remote_score;
    Ok(OptimizerTrial {
        candidate_id: candidate.id.clone(),
        task_id: candidate.bb_task.clone(),
        split: split_name.to_string(),
        score,
        source_key_recall: split_score,
        remote_verdict_score: remote_score,
        cost_usd: receipt.get("cost_usd").and_then(Value::as_f64),
        wall_ms: receipt.get("wall_ms").and_then(Value::as_u64),
        request_path,
        receipt_path,
        result_path,
        receipt,
    })
}

fn build_optimizer_sprite_request(
    eval: &EvalInfo,
    options: &OptimizerLoopOptions,
    candidate: &OptimizerCandidate,
    split_name: &str,
    split_tasks: &[String],
    target: &Value,
    submission: Option<&SubmissionRef>,
) -> Value {
    let trial_id = format!(
        "{}:{}:{}:{}",
        eval.id, candidate.id, candidate.bb_task, split_name
    );
    let threshold_submission_id = format!("threshold-{}", safe_id(&trial_id));
    let bb_submission_id = submission
        .map(|s| s.id.clone())
        .unwrap_or_else(|| threshold_submission_id.clone());
    let context = format!(
        "Threshold {} optimizer loop trial. Split={split_name}; eval digest={}; source trials digest={}. GEPA mutation: {}. Hypothesis: {}.",
        eval.id, eval.eval_digest, eval.trials_digest, candidate.mutation, candidate.hypothesis
    );
    json!({
        "schema": SPRITE_REQUEST_SCHEMA,
        "submission": bb_submission_id,
        "repo": options.bb_repo,
        "rev": options.bb_rev,
        "change": options.bb_change,
        "context": context,
        "experiment_id": eval.id,
        "trial_id": trial_id,
        "task_id": candidate.bb_task,
        "split": {
            "name": split_name,
            "tasks": split_tasks
        },
        "candidate": {
            "candidate_id": candidate.id,
            "parent_id": candidate.parent_id,
            "composition_hash": format_hash(&candidate.composition_hash),
            "model": candidate.model,
            "thinking": Value::Null,
            "prompt_packet_digest": candidate.prompt_packet_digest,
            "tool_policy": "bitterblossom-correctness",
            "gepa_mutation": {
                "name": candidate.mutation,
                "hypothesis": candidate.hypothesis
            }
        },
        "workspace": {
            "harbor_package": eval.arena_dir.as_ref().map(|p| path_string(p)),
            "candidate_visible_paths": ["RUN.json", "EVENT.json", "PR.diff", "environment/"],
            "hidden_paths": ["tests/", "solution/"]
        },
        "output_contract": "REPORT.json or result.md containing a Bitterblossom correctness verdict; Threshold scores deterministic key recall locally.",
        "secret_names": ["OPENROUTER_API_KEY", "GH_TOKEN"],
        "env_allowlist": ["OPENROUTER_API_KEY", "GH_TOKEN"],
        "budget": {
            "max_cost_usd": SPRITE_TRIAL_MAX_COST_USD,
            "timeout_seconds": 2700
        },
        "threshold": {
            "target_schema": TARGET_SCHEMA,
            "loop_schema": OPTIMIZER_LOOP_SCHEMA,
            "eval_id": eval.id,
            "eval_digest": eval.eval_digest,
            "source_trials_digest": eval.trials_digest,
            "score_owner": "threshold",
            "target": target
        },
        "threshold_submission": {
            "id": threshold_submission_id,
            "bb_submission": submission.map(|s| s.id.clone()),
            "bb_change_key": submission.map(|s| s.change_key.clone()),
            "bb_submission_row": submission.map(|s| s.row.clone()),
            "repo": options.bb_repo,
            "rev": options.bb_rev,
            "change": options.bb_change,
            "context": format!("Threshold backlog 061 GEPA/ASHA optimizer candidate trial for Crucible {}", eval.id)
        }
    })
}

fn open_bitterblossom_submission(
    options: &OptimizerLoopOptions,
    candidate: &OptimizerCandidate,
    split_name: &str,
) -> Result<SubmissionRef, Box<dyn std::error::Error>> {
    let rev = options.bb_rev.as_deref().ok_or_else(|| {
        OptimizationTargetError("--bb-rev is required to open BB submissions".to_string())
    })?;
    let change_base = options
        .bb_change
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("threshold-optimizer-loop");
    let run_slug = options
        .out_dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(safe_id)
        .unwrap_or_else(|| "run".to_string());
    let change_key = safe_id(&format!(
        "{change_base}-{run_slug}-{}-{split_name}",
        candidate.id
    ));
    let context = format!(
        "Threshold optimizer candidate {} split {} for {}",
        candidate.id, split_name, options.bb_repo
    );
    let bb_bin = absolute_path(&options.bb_bin);
    let mut cmd = Command::new(bb_bin);
    if let Some(config) = &options.bb_config {
        let plane_root = absolute_path(&bb_plane_root(config));
        cmd.arg("--config").arg(&plane_root);
        if let Some(root) = bb_repo_root(&plane_root) {
            cmd.current_dir(root);
        }
    }
    cmd.arg("submit")
        .arg("open")
        .arg("--change")
        .arg(&change_key)
        .arg("--rev")
        .arg(rev)
        .arg("--context")
        .arg(&context)
        .arg("--json");
    let output = cmd.output().map_err(|err| {
        OptimizationTargetError(format!("failed to execute bb submit open: {err}"))
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(OptimizationTargetError(format!(
            "bb submit open failed: {}",
            tail(&stderr, 4000)
        ))
        .into());
    }
    let row: Value = serde_json::from_str(&stdout).map_err(|err| {
        OptimizationTargetError(format!("parse bb submit open JSON: {err}; stdout={stdout}"))
    })?;
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| OptimizationTargetError("bb submit open JSON missing id".to_string()))?
        .to_string();
    Ok(SubmissionRef {
        id,
        change_key,
        row,
    })
}

fn read_bitterblossom_result(options: &OptimizerLoopOptions, receipt: &Value) -> Value {
    let Some(run_id) = receipt
        .get("bitter_blossom_run_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Value::Null;
    };
    let bb_bin = absolute_path(&options.bb_bin);
    let mut cmd = Command::new(bb_bin);
    if let Some(config) = &options.bb_config {
        let plane_root = absolute_path(&bb_plane_root(config));
        cmd.arg("--config").arg(&plane_root);
        if let Some(root) = bb_repo_root(&plane_root) {
            cmd.current_dir(root);
        }
    }
    cmd.arg("artifacts")
        .arg("read")
        .arg(run_id)
        .arg("result.md")
        .arg("--json");
    match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                serde_json::from_str::<Value>(&stdout).unwrap_or_else(|err| {
                    json!({"status": "failed", "error": format!("parse bb artifact JSON: {err}"), "stdout_tail": tail(&stdout, 2000)})
                })
            } else {
                json!({"status": "failed", "error": tail(&stderr, 4000), "stdout_tail": tail(&stdout, 2000)})
            }
        }
        Err(err) => {
            json!({"status": "failed", "error": format!("failed to execute bb artifacts read: {err}")})
        }
    }
}

fn remote_verdict_score(receipt: &Value, artifact: &Value) -> f64 {
    match receipt.get("status").and_then(Value::as_str) {
        Some("ok") => {}
        Some("not_dispatched") => return 1.0,
        _ => return 0.0,
    }
    let content = artifact
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Some(score) = json_verdict_score(content) {
        return score;
    }
    let content = content.to_ascii_lowercase();
    if content.contains("\"verdict\": \"pass\"") || content.contains("\"verdict\":\"pass\"") {
        1.0
    } else if content.contains("\"verdict\": \"advisory\"")
        || content.contains("\"verdict\":\"advisory\"")
        || content.contains("\"severity\": \"advisory\"")
        || content.contains("\"severity\":\"advisory\"")
    {
        0.5
    } else {
        0.0
    }
}

fn json_verdict_score(markdown: &str) -> Option<f64> {
    let trimmed = markdown.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Some(score) = verdict_score_from_value(&value) {
            return Some(score);
        }
    }

    let mut rest = markdown;
    while let Some(fence_start) = rest.find("```") {
        let after_ticks = &rest[fence_start + 3..];
        let Some(line_end) = after_ticks.find('\n') else {
            break;
        };
        let lang = after_ticks[..line_end].trim().to_ascii_lowercase();
        let after_lang = &after_ticks[line_end + 1..];
        let Some(fence_end) = after_lang.find("```") else {
            break;
        };
        if lang == "json" || lang.starts_with("json ") {
            let json_text = after_lang[..fence_end].trim();
            if let Ok(value) = serde_json::from_str::<Value>(json_text) {
                if let Some(score) = verdict_score_from_value(&value) {
                    return Some(score);
                }
            }
        }
        rest = &after_lang[fence_end + 3..];
    }

    None
}

fn verdict_score_from_value(value: &Value) -> Option<f64> {
    let verdict = value
        .get("verdict")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let severity = value
        .get("severity")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);

    if verdict.as_deref() == Some("pass") {
        Some(1.0)
    } else if verdict.as_deref() == Some("advisory") || severity.as_deref() == Some("advisory") {
        Some(0.5)
    } else if verdict.as_deref() == Some("block") || severity.as_deref() == Some("blocking") {
        Some(0.0)
    } else {
        None
    }
}

fn split_key_recall(summary: &CandidateSummary, tasks: &[String]) -> Option<f64> {
    let task_set: BTreeSet<&str> = tasks.iter().map(String::as_str).collect();
    let mut successes = 0_u64;
    let mut n = 0_u64;
    for result in &summary.task_results {
        let Some(task) = result.get("task_id").and_then(Value::as_str) else {
            continue;
        };
        if !task_set.contains(task) {
            continue;
        }
        successes += result.get("successes").and_then(Value::as_u64).unwrap_or(0);
        n += result.get("n").and_then(Value::as_u64).unwrap_or(0);
    }
    (n > 0).then_some(successes as f64 / n as f64)
}

fn history_entry(rung: u64, split: &str, trial: &OptimizerTrial) -> Value {
    json!({
        "schema": OPTIMIZER_LOOP_SCHEMA,
        "rung": rung,
        "split": split,
        "candidate_id": trial.candidate_id,
        "task_id": trial.task_id,
        "score": trial.score,
        "cost_usd": trial.cost_usd,
        "remote_verdict_score": trial.remote_verdict_score,
        "source_key_recall": trial.source_key_recall,
        "status": trial.receipt.get("status").cloned().unwrap_or(Value::Null),
        "bitter_blossom_run_id": trial.receipt.get("bitter_blossom_run_id").cloned().unwrap_or(Value::Null)
    })
}

fn build_asha(
    eval: &EvalInfo,
    options: &OptimizerLoopOptions,
    validation_trials: &[Value],
    promoted: &[Value],
    certification_trials: &[Value],
) -> Value {
    json!({
        "schema": ASHA_SCHEMA,
        "eval_id": eval.id,
        "budget_usd": options.budget_usd,
        "allocator": "hyperband-asha",
        "rungs": [
            {
                "rung": 0,
                "name": "validation",
                "budget_units": 1,
                "candidate_count": validation_trials.len(),
                "trials": validation_trials
            },
            {
                "rung": 1,
                "name": "heldout",
                "budget_units": 1,
                "promotion_rule": "promote non-dominated validation frontier by score desc, cost asc",
                "promoted": promoted,
                "trials": certification_trials
            }
        ],
        "stop_rule": {
            "known_spend_cap_usd": options.budget_usd,
            "guard": "do not allocate heldout budget outside promoted frontier"
        }
    })
}

fn build_pareto(eval: &EvalInfo, candidates: &[Value], frontier: &[Value]) -> Value {
    json!({
        "schema": PARETO_SCHEMA,
        "eval_id": eval.id,
        "objective": {
            "maximize": "score",
            "minimize": "cost_usd"
        },
        "candidates": candidates,
        "frontier": frontier
    })
}

fn build_certification(
    eval: &EvalInfo,
    splits: &TaskSplits,
    promoted: &[Value],
    certification_trials: &[Value],
) -> Value {
    json!({
        "schema": CERTIFICATION_SCHEMA,
        "eval_id": eval.id,
        "heldout_tasks": splits.heldout,
        "status": "heldout_trial_recorded_seed_trust_not_certified",
        "policy": "heldout is used only after ASHA promotion and is not fed back into GEPA mutation in this run; multi-seed stability remains required before launch certification",
        "promoted_from_validation": promoted,
        "heldout_trials": certification_trials
    })
}

fn build_optimizer_guardrails(
    eval: &EvalInfo,
    headroom: &Value,
    splits: &TaskSplits,
    receipts: &[Value],
) -> Value {
    let spend = known_receipt_spend(receipts);
    json!({
        "schema": GUARDRAILS_SCHEMA,
        "eval_id": eval.id,
        "overfitting": {
            "status": "pass",
            "validation_tasks": splits.validation,
            "heldout_tasks": splits.heldout,
            "holdout_feedback_blocked": true,
            "evidence": "GEPA candidates are generated before heldout trials; heldout receipts are recorded only in certification.json."
        },
        "judge_gaming": {
            "status": "partial",
            "evidence": "Primary score remains deterministic key recall from Threshold source trials; BB/Sprites verdict is a remote execution gate, not a standalone judge objective."
        },
        "non_stationarity": {
            "status": "pass",
            "eval_digest": eval.eval_digest,
            "source_trials_digest": eval.trials_digest,
            "model_provider_ids_recorded": true
        },
        "budget": {
            "known_spend_usd": spend,
            "receipt_count": receipts.len()
        },
        "seed_trust": {
            "status": "not_certified",
            "evidence": "This first optimizer-loop slice records rng_seed and a single seeded trajectory; backlog 057 multi-seed optimizer-vs-seed-scan remains required before a final recommendation."
        },
        "headroom_verdict": headroom.get("verdict").cloned().unwrap_or(Value::Null)
    })
}

fn pareto_frontier(candidates: &[Value]) -> Vec<Value> {
    let mut frontier = Vec::new();
    for candidate in candidates {
        if candidates.iter().any(|other| dominates(other, candidate)) {
            continue;
        }
        frontier.push(candidate.clone());
    }
    frontier.sort_by(compare_candidates_for_promotion);
    frontier
}

fn dominates(a: &Value, b: &Value) -> bool {
    let a_score = a
        .get("score")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY);
    let b_score = b
        .get("score")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY);
    let a_cost = cost_for_ordering(a);
    let b_cost = cost_for_ordering(b);
    (a_score >= b_score && a_cost <= b_cost) && (a_score > b_score || a_cost < b_cost)
}

fn compare_candidates_for_promotion(a: &Value, b: &Value) -> std::cmp::Ordering {
    let a_score = a
        .get("score")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY);
    let b_score = b
        .get("score")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY);
    b_score
        .partial_cmp(&a_score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            cost_for_ordering(a)
                .partial_cmp(&cost_for_ordering(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| {
            a.get("candidate_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(b.get("candidate_id").and_then(Value::as_str).unwrap_or(""))
        })
}

fn cost_for_ordering(value: &Value) -> f64 {
    value
        .get("cost_usd")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY)
}

fn known_receipt_spend(receipts: &[Value]) -> Option<f64> {
    receipts.iter().try_fold(0.0_f64, |acc, receipt| {
        let status = receipt.get("status").and_then(Value::as_str);
        if status == Some("not_dispatched") {
            Some(acc)
        } else {
            receipt
                .get("cost_usd")
                .and_then(Value::as_f64)
                .map(|v| acc + v)
        }
    })
}

fn write_history(path: &Path, entries: &[Value]) -> Result<(), Box<dyn std::error::Error>> {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&serde_json::to_string(entry)?);
        text.push('\n');
    }
    std::fs::write(path, text)?;
    Ok(())
}

fn render_optimizer_report(
    eval: &EvalInfo,
    options: &OptimizerLoopOptions,
    splits: &TaskSplits,
    validation_trials: &[Value],
    frontier: &[Value],
    certification_trials: &[Value],
    headroom: &Value,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Optimizer Loop: {}\n\n", eval.id));
    out.push_str(&format!(
        "- Headroom verdict: `{}`\n",
        headroom
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));
    out.push_str(&format!("- Budget cap: `${:.2}`\n", options.budget_usd));
    out.push_str(&format!(
        "- Validation tasks: `{}`\n",
        splits.validation.join(", ")
    ));
    out.push_str(&format!(
        "- Heldout tasks: `{}`\n",
        splits.heldout.join(", ")
    ));
    out.push_str("\n## Validation Population\n\n");
    out.push_str("| candidate | bb task | score | source recall | remote gate | cost | run |\n");
    out.push_str("|---|---|---:|---:|---:|---:|---|\n");
    for trial in validation_trials {
        out.push_str(&trial_report_row(trial));
    }
    out.push_str("\n## Pareto Frontier\n\n");
    out.push_str("| candidate | score | cost |\n");
    out.push_str("|---|---:|---:|\n");
    for trial in frontier {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            trial
                .get("candidate_id")
                .and_then(Value::as_str)
                .unwrap_or(""),
            fmt_opt4(trial.get("score").and_then(Value::as_f64)),
            fmt_money(trial.get("cost_usd").and_then(Value::as_f64))
        ));
    }
    out.push_str("\n## Heldout Certification\n\n");
    out.push_str("| candidate | bb task | score | source recall | remote gate | cost | run |\n");
    out.push_str("|---|---|---:|---:|---:|---:|---|\n");
    for trial in certification_trials {
        out.push_str(&trial_report_row(trial));
    }
    out.push_str("\n## Guardrail Read\n\n");
    out.push_str("- GEPA mutations are recorded before heldout certification and heldout results are not fed back into this run.\n");
    out.push_str("- The score formula is deterministic key recall multiplied by the BB/Sprites verdict gate. This keeps Threshold as scorer while the remote plane supplies execution evidence.\n");
    out.push_str("- Seed trust is not certified by this first run; run the multi-seed 057 check before any launch recommendation.\n");
    out.push_str("- Crucible grade parity remains a caveat until the Crucible scorer matches Threshold's Rust scorer.\n");
    out
}

fn trial_report_row(trial: &Value) -> String {
    format!(
        "| {} | {} | {} | {} | {} | {} | {} |\n",
        trial
            .get("candidate_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
        trial.get("task_id").and_then(Value::as_str).unwrap_or(""),
        fmt_opt4(trial.get("score").and_then(Value::as_f64)),
        fmt_opt4(
            trial
                .get("score_source")
                .and_then(|s| s.get("source_key_recall"))
                .and_then(Value::as_f64)
        ),
        fmt_opt4(
            trial
                .get("score_source")
                .and_then(|s| s.get("remote_verdict_score"))
                .and_then(Value::as_f64)
        ),
        fmt_money(trial.get("cost_usd").and_then(Value::as_f64)),
        trial
            .get("bitter_blossom_run_id")
            .and_then(Value::as_str)
            .unwrap_or("")
    )
}

fn model_for_bb_task(task: &str) -> String {
    if task.contains("glm") {
        "z-ai/glm-5.2".to_string()
    } else if task.contains("kimi") {
        "moonshotai/kimi-k2.7-code".to_string()
    } else {
        "deepseek/deepseek-v4-pro".to_string()
    }
}

fn safe_id(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "item".to_string()
    } else {
        trimmed
    }
}

fn build_sprite_request(
    eval: &EvalInfo,
    options: &HeadroomProbeOptions,
    summaries: &[CandidateSummary],
    target: &Value,
) -> Value {
    let incumbent = summaries
        .iter()
        .find(|s| s.candidate_id == eval.candidate_id)
        .or_else(|| {
            summaries
                .iter()
                .find(|s| !is_costless_kind(&s.candidate_kind))
        });
    let composition_hash = incumbent
        .map(|s| s.composition_hash.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sha256_hex(format!("{}:{}", eval.id, options.bb_task).as_bytes()));
    let model = incumbent
        .and_then(|s| s.model.clone())
        .unwrap_or_else(|| "deepseek/deepseek-v4-pro".to_string());
    let trial_id = format!(
        "{}:{}:headroom-probe:{}",
        eval.id,
        options.bb_task,
        short_hash(&composition_hash)
    );
    let threshold_submission_id = format!("threshold-{trial_id}");
    let bb_submission_id = options
        .bb_submission
        .clone()
        .unwrap_or_else(|| threshold_submission_id.clone());
    let context = format!(
        "Threshold {} headroom/Sprites seam probe. Eval digest: {}; source trials digest: {}. Threshold-specific trial contract is embedded in this EVENT.json under schema={}.",
        eval.id, eval.eval_digest, eval.trials_digest, SPRITE_REQUEST_SCHEMA
    );
    json!({
        "schema": SPRITE_REQUEST_SCHEMA,
        "submission": bb_submission_id,
        "repo": options.bb_repo,
        "rev": options.bb_rev,
        "change": options.bb_change,
        "context": context,
        "experiment_id": eval.id,
        "trial_id": trial_id,
        "task_id": options.bb_task,
        "candidate": {
            "composition_hash": format_hash(&composition_hash),
            "model": model,
            "thinking": Value::Null,
            "prompt_packet_digest": target.get("eval").and_then(|e| e.get("digest")).cloned().unwrap_or(Value::Null),
            "tool_policy": "bitterblossom-correctness"
        },
        "workspace": {
            "harbor_package": eval.arena_dir.as_ref().map(|p| path_string(p)),
            "candidate_visible_paths": ["RUN.json", "EVENT.json", "PR.diff", "environment/"],
            "hidden_paths": ["tests/", "solution/"]
        },
        "output_contract": "REPORT.json containing the Bitterblossom correctness verdict JSON; Threshold scores returned findings locally when available",
        "secret_names": ["OPENROUTER_API_KEY", "GH_TOKEN"],
        "env_allowlist": ["OPENROUTER_API_KEY", "GH_TOKEN"],
        "budget": {
            "max_cost_usd": SPRITE_TRIAL_MAX_COST_USD,
            "timeout_seconds": 2700
        },
        "threshold": {
            "target_schema": TARGET_SCHEMA,
            "eval_id": eval.id,
            "eval_digest": eval.eval_digest,
            "source_trials_digest": eval.trials_digest,
            "score_owner": "threshold"
        },
        "threshold_submission": {
            "id": threshold_submission_id,
            "bb_submission": options.bb_submission,
            "repo": options.bb_repo,
            "rev": options.bb_rev,
            "change": options.bb_change,
            "context": format!("Threshold backlog 061 first Sprites runner seam probe for Crucible {}", eval.id)
        }
    })
}

fn dispatch_bitterblossom(
    options: &HeadroomProbeOptions,
    request_path: &Path,
    request: &Value,
) -> Value {
    let started = now_iso();
    let payload_path = absolute_path(request_path);
    let idempotency = format!(
        "threshold-{}-{}",
        request
            .get("trial_id")
            .and_then(Value::as_str)
            .unwrap_or("trial")
            .replace([':', '/'], "-"),
        short_hash(&sha256_hex(
            serde_json::to_string(request)
                .unwrap_or_default()
                .as_bytes()
        ))
    );
    let bb_bin = absolute_path(&options.bb_bin);
    let mut cmd = Command::new(bb_bin);
    if let Some(config) = &options.bb_config {
        let plane_root = absolute_path(&bb_plane_root(config));
        cmd.arg("--config").arg(&plane_root);
        if let Some(root) = bb_repo_root(&plane_root) {
            cmd.current_dir(root);
        }
    }
    cmd.arg("run")
        .arg(&options.bb_task)
        .arg("--idempotency-key")
        .arg(&idempotency)
        .arg("--payload-file")
        .arg(&payload_path)
        .arg("--json");

    let result = cmd.output();
    let ended = now_iso();
    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout_json = redact_local_paths_in_value(
                serde_json::from_str::<Value>(&stdout).unwrap_or(Value::Null),
            );
            let run_id = extract_bb_run_id(&stdout_json);
            let bb_run = stdout_json.get("run").unwrap_or(&Value::Null);
            let last_attempt = stdout_json
                .get("attempts")
                .and_then(Value::as_array)
                .and_then(|a| a.last());
            let model_served = last_attempt
                .and_then(|a| a.get("model"))
                .cloned()
                .or_else(|| {
                    request
                        .get("candidate")
                        .and_then(|c| c.get("model"))
                        .cloned()
                })
                .unwrap_or(Value::Null);
            let status = if output.status.success() {
                "ok"
            } else {
                "failed"
            };
            let digest_basis = format!("{stdout}\n---stderr---\n{stderr}");
            json!({
                "schema": SPRITE_RECEIPT_SCHEMA,
                "trial_id": request.get("trial_id").cloned().unwrap_or(Value::Null),
                "task_id": request.get("task_id").cloned().unwrap_or(Value::Null),
                "composition_hash": request.get("candidate").and_then(|c| c.get("composition_hash")).cloned().unwrap_or(Value::Null),
                "bitter_blossom_run_id": run_id,
                "substrate": "sprites",
                "status": status,
                "artifact_refs": {
                    "findings": Value::Null,
                    "transcript": Value::Null,
                    "bb_stdout": stdout_json
                },
                "model_served": model_served,
                "tokens_prompt": last_attempt.and_then(|a| a.get("tokens_in")).cloned().unwrap_or(Value::Null),
                "tokens_completion": last_attempt.and_then(|a| a.get("tokens_out")).cloned().unwrap_or(Value::Null),
                "cost_usd": bb_run.get("cost_usd").cloned().unwrap_or(Value::Null),
                "wall_ms": bb_run.get("duration_ms").cloned().unwrap_or(Value::Null),
                "error": if output.status.success() {
                    Value::Null
                } else {
                    json!({
                        "exit_code": output.status.code(),
                        "stderr_tail": tail(&stderr, 4000),
                        "stdout_tail": tail(&stdout, 4000)
                    })
                },
                "command": {
                    "bb_bin": path_string(&options.bb_bin),
                    "config": options.bb_config.as_ref().map(|p| path_string(p)),
                    "payload_file": path_string(&payload_path),
                    "task": options.bb_task,
                    "idempotency_key": idempotency
                },
                "started_at": started,
                "ended_at": ended,
                "receipt_digest": format!("sha256:{}", sha256_hex(digest_basis.as_bytes()))
            })
        }
        Err(err) => json!({
            "schema": SPRITE_RECEIPT_SCHEMA,
            "trial_id": request.get("trial_id").cloned().unwrap_or(Value::Null),
            "task_id": request.get("task_id").cloned().unwrap_or(Value::Null),
            "composition_hash": request.get("candidate").and_then(|c| c.get("composition_hash")).cloned().unwrap_or(Value::Null),
            "bitter_blossom_run_id": Value::Null,
            "substrate": "sprites",
            "status": "failed",
            "artifact_refs": {
                "findings": Value::Null,
                "transcript": Value::Null
            },
            "model_served": request.get("candidate").and_then(|c| c.get("model")).cloned().unwrap_or(Value::Null),
            "tokens_prompt": Value::Null,
            "tokens_completion": Value::Null,
            "cost_usd": Value::Null,
            "wall_ms": Value::Null,
            "error": format!("failed to execute bb: {err}"),
            "started_at": started,
            "ended_at": ended,
            "receipt_digest": format!("sha256:{}", sha256_hex(err.to_string().as_bytes()))
        }),
    }
}

fn build_pending_receipt(request: &Value) -> Value {
    json!({
        "schema": SPRITE_RECEIPT_SCHEMA,
        "trial_id": request.get("trial_id").cloned().unwrap_or(Value::Null),
        "task_id": request.get("task_id").cloned().unwrap_or(Value::Null),
        "composition_hash": request.get("candidate").and_then(|c| c.get("composition_hash")).cloned().unwrap_or(Value::Null),
        "bitter_blossom_run_id": Value::Null,
        "substrate": "sprites",
        "status": "not_dispatched",
        "artifact_refs": {
            "findings": Value::Null,
            "transcript": Value::Null
        },
        "model_served": request.get("candidate").and_then(|c| c.get("model")).cloned().unwrap_or(Value::Null),
        "tokens_prompt": Value::Null,
        "tokens_completion": Value::Null,
        "cost_usd": Value::Null,
        "wall_ms": 0,
        "error": "run with --dispatch-bitterblossom to call bb run",
        "receipt_digest": Value::Null
    })
}

fn render_report(
    eval: &EvalInfo,
    options: &HeadroomProbeOptions,
    summaries: &[CandidateSummary],
    headroom: &Value,
    receipt: &Value,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Optimizer Headroom Probe: {}\n\n", eval.id));
    out.push_str(&format!(
        "- Verdict: `{}`\n",
        headroom
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));
    out.push_str(&format!("- Budget cap: `${:.2}`\n", options.budget_usd));
    out.push_str(&format!("- Crucible eval digest: `{}`\n", eval.eval_digest));
    out.push_str(&format!(
        "- Source trials digest: `{}`\n",
        eval.trials_digest
    ));
    out.push_str("\n| candidate | key recall | Wilson 95% CI | defects | reward mean | known cost | wall |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for summary in summaries {
        out.push_str(&format!(
            "| {} | {} | {} | {}/{} | {} | {} | {} ms |\n",
            summary.candidate_id,
            fmt_opt4(summary.point),
            fmt_ci(summary.lower, summary.upper),
            summary.successes,
            summary.n,
            fmt_opt4(summary.reward_mean),
            summary
                .cost_usd_total
                .map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "unknown".to_string()),
            summary.wall_ms_total
        ));
    }
    out.push_str("\n## Sprites Dispatch\n\n");
    out.push_str(&format!(
        "- Requested: `{}`\n",
        options.dispatch_bitterblossom
    ));
    out.push_str(&format!(
        "- Receipt status: `{}`\n",
        receipt
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));
    if let Some(run_id) = receipt
        .get("bitter_blossom_run_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("- Bitterblossom run id: `{run_id}`\n"));
    }
    if let Some(error) = receipt.get("error") {
        if !error.is_null() {
            out.push_str(&format!("- Error: `{}`\n", compact_json(error)));
        }
    }
    out.push_str("\n## Guardrail Read\n\n");
    out.push_str("- This slice uses deterministic key-recall evidence; no judge-only objective decides a winner.\n");
    out.push_str("- Full Crucible Harbor bundle digests are not present in this eval spec, so this is a transitional target import, not a final G2 approval packet.\n");
    out.push_str("- Search remains blocked until G1/G2 approval; the probe only establishes whether the target has headroom.\n");
    out
}

fn candidate_to_value(summary: &CandidateSummary) -> Value {
    json!({
        "candidate_id": summary.candidate_id,
        "candidate_kind": summary.candidate_kind,
        "composition_hash": summary.composition_hash,
        "model": summary.model,
        "successes": summary.successes,
        "n": summary.n,
        "point": summary.point,
        "lower": summary.lower,
        "upper": summary.upper,
        "expected_defects": summary.expected_defects,
        "false_positives": summary.false_positives,
        "reward_mean": summary.reward_mean,
        "cost_usd_total": summary.cost_usd_total,
        "wall_ms_total": summary.wall_ms_total,
        "trials": summary.trials,
        "task_results": summary.task_results
    })
}

fn write_trials_jsonl(
    path: &Path,
    records: &[Value],
    eval: &EvalInfo,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut text = String::new();
    for record in records {
        let mut cloned = record.clone();
        if let Some(obj) = cloned.as_object_mut() {
            obj.insert(
                "optimization_eval_id".to_string(),
                Value::String(eval.id.clone()),
            );
            obj.insert(
                "optimization_eval_digest".to_string(),
                Value::String(eval.eval_digest.clone()),
            );
        }
        text.push_str(&serde_json::to_string(&cloned)?);
        text.push('\n');
    }
    std::fs::write(path, text)?;
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

fn read_jsonl_values(path: &Path) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| OptimizationTargetError(format!("read {}: {err}", path.display())))?;
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str::<Value>(line).map_err(|err| {
            OptimizationTargetError(format!("parse {} line {}: {err}", path.display(), idx + 1))
        })?);
    }
    Ok(out)
}

fn candidate_ids(eval: &EvalInfo) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for candidate in eval
        .baselines
        .iter()
        .chain(std::iter::once(&eval.candidate_id))
    {
        if seen.insert(candidate.clone()) {
            out.push(candidate.clone());
        }
    }
    out
}

fn required_str<'a>(
    value: &'a Value,
    path: &[&str],
    label: &str,
) -> Result<&'a str, OptimizationTargetError> {
    let mut current = value;
    for segment in path {
        current = current
            .get(*segment)
            .ok_or_else(|| OptimizationTargetError(format!("{label} is required")))?;
    }
    current
        .as_str()
        .ok_or_else(|| OptimizationTargetError(format!("{label} must be string")))
}

fn required_str_in<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, OptimizationTargetError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| OptimizationTargetError(format!("runner.corpus.{key} is required")))
}

fn resolve_existing(base: &Path, raw: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    std::fs::canonicalize(&joined).map_err(|err| {
        OptimizationTargetError(format!(
            "resolve {} relative to {}: {err}",
            raw,
            base.display()
        ))
        .into()
    })
}

fn is_costless_kind(kind: &str) -> bool {
    matches!(kind, "oracle" | "null")
}

fn wilson_interval(successes: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let z = 1.959_963_984_540_054_f64;
    let nf = n as f64;
    let phat = successes as f64 / nf;
    let z2 = z * z;
    let denom = 1.0 + z2 / nf;
    let center = (phat + z2 / (2.0 * nf)) / denom;
    let margin = z * ((phat * (1.0 - phat) + z2 / (4.0 * nf)) / nf).sqrt() / denom;
    ((center - margin).max(0.0), (center + margin).min(1.0))
}

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes.as_ref());
    format!("{:x}", hasher.finalize())
}

fn short_hash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(12)
        .collect()
}

fn format_hash(hash: &str) -> String {
    if hash.starts_with("sha256:") {
        hash.to_string()
    } else {
        format!("sha256:{hash}")
    }
}

fn path_string(path: &Path) -> String {
    redact_local_paths(&path.to_string_lossy())
}

fn redact_local_paths_in_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_local_paths(&s)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_local_paths_in_value).collect())
        }
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, redact_local_paths_in_value(value)))
                .collect(),
        ),
        other => other,
    }
}

fn redact_local_paths(text: &str) -> String {
    redact_development_root(
        &redact_development_root(
            &redact_development_root(text, "threshold", "$THRESHOLD_REPO"),
            "crucible-evals",
            "$CRUCIBLE_EVALS_REPO",
        ),
        "bitterblossom",
        "$BITTERBLOSSOM_REPO",
    )
}

fn redact_development_root(text: &str, repo: &str, replacement: &str) -> String {
    let marker = format!("/Development/{repo}");
    let Some(index) = text.find(&marker) else {
        return text.to_string();
    };
    let path_start = text[..index]
        .rfind(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ':' | '='))
        .map(|i| i + 1)
        .unwrap_or(0);
    format!(
        "{}{}{}",
        &text[..path_start],
        replacement,
        &text[index + marker.len()..]
    )
}

fn now_iso() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            let secs = d.as_secs() as i64;
            let days = secs.div_euclid(86_400);
            let rem = secs.rem_euclid(86_400);
            let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
            let (y, mo, dy) = crate::pycompat::civil_from_days(days);
            format!("{y:04}-{mo:02}-{dy:02}T{h:02}:{m:02}:{s:02}+00:00")
        })
        .unwrap_or_else(|_| "1970-01-01T00:00:00+00:00".to_string())
}

fn bb_plane_root(config: &Path) -> PathBuf {
    if config.file_name().and_then(|s| s.to_str()) == Some("plane.toml") {
        config
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| config.to_path_buf())
    } else {
        config.to_path_buf()
    }
}

fn bb_repo_root(plane_root: &Path) -> Option<PathBuf> {
    plane_root.parent().map(Path::to_path_buf)
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    })
}

fn extract_bb_run_id(stdout_json: &Value) -> Value {
    for key in ["run_id", "id"] {
        if let Some(v) = stdout_json.get(key).cloned() {
            return v;
        }
    }
    if let Some(v) = stdout_json
        .get("run")
        .and_then(|r| r.get("id").or_else(|| r.get("run_id")))
        .cloned()
    {
        return v;
    }
    Value::Null
}

fn tail(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
}

fn fmt_opt4(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.4}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn fmt_ci(lo: Option<f64>, hi: Option<f64>) -> String {
    match (lo, hi) {
        (Some(lo), Some(hi)) => format!("[{lo:.4}, {hi:.4}]"),
        _ => "n/a".to_string(),
    }
}

fn fmt_money(v: Option<f64>) -> String {
    v.map(|x| format!("${x:.4}"))
        .unwrap_or_else(|| "unknown".to_string())
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "unprintable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_key_recall_eval_and_writes_headroom_probe() {
        let root = fresh_tmp("import");
        let eval_dir = root.join("evals");
        let run_dir = root.join("runs").join("freeze");
        std::fs::create_dir_all(&eval_dir).unwrap();
        std::fs::create_dir_all(&run_dir).unwrap();
        let trials = run_dir.join("trials.jsonl");
        std::fs::write(
            &trials,
            [
                TrialRow::new("oracle", "oracle", "t1", 2, &["a", "b"]).to_jsonl(),
                TrialRow::new("oracle", "oracle", "t2", 0, &[])
                    .reward(1.0)
                    .to_jsonl(),
                TrialRow::new("null", "null", "t1", 2, &[]).to_jsonl(),
                TrialRow::new("null", "null", "t2", 0, &[])
                    .reward(1.0)
                    .to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t1", 2, &["a"])
                    .false_positives(1)
                    .reward(0.3)
                    .cost(0.25)
                    .to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t1", 2, &[])
                    .reward(0.0)
                    .cost(0.02)
                    .to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t2", 0, &[])
                    .reward(1.0)
                    .cost(0.05)
                    .to_jsonl(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let eval = eval_dir.join("pr-review-key-recall-v0.json");
        std::fs::write(
            &eval,
            json!({
                "schema_version": CRUCIBLE_EVAL_SCHEMA,
                "id": "pr-review-key-recall-v0",
                "task": "pr-review-key-recall",
                "baselines": ["null", "oracle"],
                "aggregation": "proportion",
                "runner": {
                    "kind": "key_recall",
                    "corpus": {
                        "trials_jsonl": "../runs/freeze/trials.jsonl",
                        "candidate_id": "probe-oneshot",
                        "tasks": ["t1", "t2"]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        let out_dir = root.join("out");
        let result = run_headroom_probe(&HeadroomProbeOptions {
            eval_spec: eval,
            out_dir: out_dir.clone(),
            budget_usd: 5.0,
            bb_config: None,
            bb_task: "correctness".to_string(),
            bb_bin: PathBuf::from("bb"),
            bb_repo: "misty-step/threshold".to_string(),
            bb_rev: Some("abc123".to_string()),
            bb_change: Some("threshold-optimizer-061".to_string()),
            bb_submission: Some("bb-submission-123".to_string()),
            dispatch_bitterblossom: false,
        })
        .unwrap();
        assert_eq!(result.verdict, "pass");
        assert_eq!(result.probe_point, Some(0.25));
        let headroom: Value =
            serde_json::from_str(&std::fs::read_to_string(result.headroom_probe).unwrap()).unwrap();
        assert_eq!(headroom["schema"], HEADROOM_SCHEMA);
        assert_eq!(headroom["checks"]["oracle_reaches_ceiling"], true);
        assert_eq!(headroom["checks"]["oneshot_not_saturated"], true);
        assert_eq!(headroom["spend_known_usd"], json!(0.32));
        assert_eq!(headroom["candidates"][2]["successes"], json!(1));
        assert_eq!(headroom["candidates"][2]["n"], json!(4));
        assert_eq!(headroom["candidates"][2]["trials"], json!(3));
        let target: Value =
            serde_json::from_str(&std::fs::read_to_string(result.target).unwrap()).unwrap();
        assert_eq!(target["schema"], TARGET_SCHEMA);
        let request: Value =
            serde_json::from_str(&std::fs::read_to_string(result.sprite_request).unwrap()).unwrap();
        assert_eq!(request["schema"], SPRITE_REQUEST_SCHEMA);
        assert_eq!(request["submission"], "bb-submission-123");
        assert_eq!(
            request["threshold_submission"]["bb_submission"],
            "bb-submission-123"
        );
        assert_eq!(request["repo"], "misty-step/threshold");
        assert_eq!(request["rev"], "abc123");
        let receipt: Value =
            serde_json::from_str(&std::fs::read_to_string(result.sprite_receipt).unwrap()).unwrap();
        assert_eq!(receipt["status"], "not_dispatched");
    }

    #[test]
    fn saturated_probe_returns_saturated_verdict() {
        let root = fresh_tmp("saturated");
        let eval_dir = root.join("evals");
        let run_dir = root.join("runs");
        std::fs::create_dir_all(&eval_dir).unwrap();
        std::fs::create_dir_all(&run_dir).unwrap();
        let trials = run_dir.join("trials.jsonl");
        std::fs::write(
            &trials,
            [
                TrialRow::new("oracle", "oracle", "t1", 2, &["a", "b"]).to_jsonl(),
                TrialRow::new("null", "null", "t1", 2, &[]).to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t1", 2, &["a", "b"])
                    .reward(1.0)
                    .cost(0.1)
                    .to_jsonl(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let eval = eval_dir.join("eval.json");
        std::fs::write(
            &eval,
            json!({
                "schema_version": CRUCIBLE_EVAL_SCHEMA,
                "id": "eval",
                "baselines": ["null", "oracle"],
                "runner": {"kind": "key_recall", "corpus": {
                    "trials_jsonl": "../runs/trials.jsonl",
                    "candidate_id": "probe-oneshot",
                    "tasks": ["t1"]
                }}
            })
            .to_string(),
        )
        .unwrap();
        let result = run_headroom_probe(&HeadroomProbeOptions {
            eval_spec: eval,
            out_dir: root.join("out"),
            budget_usd: 5.0,
            bb_config: None,
            bb_task: "correctness".to_string(),
            bb_bin: PathBuf::from("bb"),
            bb_repo: "misty-step/threshold".to_string(),
            bb_rev: Some("abc123".to_string()),
            bb_change: Some("threshold-optimizer-061".to_string()),
            bb_submission: None,
            dispatch_bitterblossom: false,
        })
        .unwrap();
        assert_eq!(result.verdict, "saturated");
    }

    #[test]
    fn broken_oracle_near_probe_returns_needs_review_not_saturated() {
        let root = fresh_tmp("broken-oracle");
        let eval_dir = root.join("evals");
        let run_dir = root.join("runs");
        std::fs::create_dir_all(&eval_dir).unwrap();
        std::fs::create_dir_all(&run_dir).unwrap();
        let trials = run_dir.join("trials.jsonl");
        std::fs::write(
            &trials,
            [
                TrialRow::new("oracle", "oracle", "t1", 2, &["a"]).to_jsonl(),
                TrialRow::new("null", "null", "t1", 2, &[]).to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t1", 2, &["a"]).to_jsonl(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let eval = eval_dir.join("eval.json");
        std::fs::write(
            &eval,
            json!({
                "schema_version": CRUCIBLE_EVAL_SCHEMA,
                "id": "eval",
                "baselines": ["null", "oracle"],
                "runner": {"kind": "key_recall", "corpus": {
                    "trials_jsonl": "../runs/trials.jsonl",
                    "candidate_id": "probe-oneshot",
                    "tasks": ["t1"]
                }}
            })
            .to_string(),
        )
        .unwrap();
        let result = run_headroom_probe(&HeadroomProbeOptions {
            eval_spec: eval,
            out_dir: root.join("out"),
            budget_usd: 5.0,
            bb_config: None,
            bb_task: "correctness".to_string(),
            bb_bin: PathBuf::from("bb"),
            bb_repo: "misty-step/threshold".to_string(),
            bb_rev: Some("abc123".to_string()),
            bb_change: Some("threshold-optimizer-061".to_string()),
            bb_submission: None,
            dispatch_bitterblossom: false,
        })
        .unwrap();
        assert_eq!(result.verdict, "needs-review");
    }

    #[test]
    fn optimizer_loop_writes_asha_pareto_and_certification() {
        let root = fresh_tmp("optimizer-loop");
        let eval_dir = root.join("evals");
        let run_dir = root.join("runs");
        std::fs::create_dir_all(&eval_dir).unwrap();
        std::fs::create_dir_all(&run_dir).unwrap();
        let trials = run_dir.join("trials.jsonl");
        std::fs::write(
            &trials,
            [
                TrialRow::new("oracle", "oracle", "t1", 2, &["a", "b"]).to_jsonl(),
                TrialRow::new("oracle", "oracle", "t2", 1, &["c"]).to_jsonl(),
                TrialRow::new("oracle", "oracle", "t3", 1, &["d"]).to_jsonl(),
                TrialRow::new("null", "null", "t1", 2, &[]).to_jsonl(),
                TrialRow::new("null", "null", "t2", 1, &[]).to_jsonl(),
                TrialRow::new("null", "null", "t3", 1, &[]).to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t1", 2, &["a"])
                    .cost(0.02)
                    .to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t2", 1, &["c"])
                    .cost(0.02)
                    .to_jsonl(),
                TrialRow::new("probe-oneshot", "oneshot", "t3", 1, &[])
                    .cost(0.02)
                    .to_jsonl(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        let eval = eval_dir.join("eval.json");
        std::fs::write(
            &eval,
            json!({
                "schema_version": CRUCIBLE_EVAL_SCHEMA,
                "id": "eval",
                "baselines": ["null", "oracle"],
                "runner": {"kind": "key_recall", "corpus": {
                    "trials_jsonl": "../runs/trials.jsonl",
                    "candidate_id": "probe-oneshot",
                    "tasks": ["t1", "t2", "t3"]
                }}
            })
            .to_string(),
        )
        .unwrap();
        let result = run_optimizer_loop(&OptimizerLoopOptions {
            eval_spec: eval.clone(),
            out_dir: root.join("out"),
            budget_usd: 5.0,
            bb_config: None,
            bb_tasks: vec![
                "correctness".to_string(),
                "correctness-glm".to_string(),
                "correctness-kimi".to_string(),
            ],
            bb_bin: PathBuf::from("bb"),
            bb_repo: "misty-step/threshold".to_string(),
            bb_rev: Some("abc123".to_string()),
            bb_change: Some("threshold-optimizer-061".to_string()),
            dispatch_bitterblossom: false,
            certify_top: 1,
            rng_seed: Some(7),
        })
        .unwrap();
        assert_eq!(result.candidates, 3);
        let asha: Value =
            serde_json::from_str(&std::fs::read_to_string(result.asha).unwrap()).unwrap();
        assert_eq!(asha["schema"], ASHA_SCHEMA);
        assert_eq!(asha["rungs"][0]["candidate_count"], json!(3));
        assert_eq!(asha["rungs"][1]["trials"].as_array().unwrap().len(), 1);
        let pareto: Value =
            serde_json::from_str(&std::fs::read_to_string(result.pareto).unwrap()).unwrap();
        assert_eq!(pareto["schema"], PARETO_SCHEMA);
        assert_eq!(pareto["candidates"].as_array().unwrap().len(), 3);
        assert!(!pareto["frontier"].as_array().unwrap().is_empty());
        let certification: Value =
            serde_json::from_str(&std::fs::read_to_string(result.certification).unwrap()).unwrap();
        assert_eq!(certification["schema"], CERTIFICATION_SCHEMA);
        assert_eq!(certification["heldout_tasks"], json!(["t3"]));
        let guardrails: Value =
            serde_json::from_str(&std::fs::read_to_string(result.guardrails).unwrap()).unwrap();
        assert_eq!(guardrails["overfitting"]["holdout_feedback_blocked"], true);

        let err = run_optimizer_loop(&OptimizerLoopOptions {
            eval_spec: eval,
            out_dir: root.join("budget-block"),
            budget_usd: 0.5,
            bb_config: None,
            bb_tasks: vec!["correctness".to_string()],
            bb_bin: PathBuf::from("bb-that-should-not-run"),
            bb_repo: "misty-step/threshold".to_string(),
            bb_rev: Some("abc123".to_string()),
            bb_change: Some("threshold-optimizer-061".to_string()),
            dispatch_bitterblossom: true,
            certify_top: 1,
            rng_seed: Some(7),
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("budget cap would be exceeded"));

        let err = run_optimizer_loop(&OptimizerLoopOptions {
            eval_spec: root.join("evals").join("eval.json"),
            out_dir: root.join("blank-task"),
            budget_usd: 5.0,
            bb_config: None,
            bb_tasks: vec![" ".to_string()],
            bb_bin: PathBuf::from("bb"),
            bb_repo: "misty-step/threshold".to_string(),
            bb_rev: Some("abc123".to_string()),
            bb_change: Some("threshold-optimizer-061".to_string()),
            dispatch_bitterblossom: false,
            certify_top: 1,
            rng_seed: Some(7),
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("--bb-task entries must not be empty"));
    }

    #[test]
    fn compact_advisory_verdict_scores_as_half_gate() {
        let receipt = json!({"status": "ok"});
        let artifact = json!({
            "content": "{\"verdict\":\"advisory\",\"findings\":[{\"severity\":\"serious\"}]}"
        });
        assert_eq!(remote_verdict_score(&receipt, &artifact), 0.5);
    }

    #[test]
    fn missing_remote_verdict_scores_as_zero_gate() {
        let receipt = json!({"status": "ok"});
        assert_eq!(remote_verdict_score(&receipt, &json!({})), 0.0);
        assert_eq!(
            remote_verdict_score(&receipt, &json!({"content": "no verdict here"})),
            0.0
        );
    }

    #[test]
    fn markdown_json_verdict_is_scored_before_fallback() {
        let receipt = json!({"status": "ok"});
        let artifact = json!({
            "content": "Review result\n\n```json\n{\"verdict\":\"pass\",\"findings\":[]}\n```"
        });
        assert_eq!(remote_verdict_score(&receipt, &artifact), 1.0);
    }

    #[test]
    fn development_paths_are_redacted_for_artifacts() {
        assert_eq!(
            redact_local_paths("/tmp/Development/threshold/runs/x"),
            "$THRESHOLD_REPO/runs/x"
        );
        assert_eq!(
            redact_local_paths("payload=/tmp/Development/bitterblossom/plane"),
            "payload=$BITTERBLOSSOM_REPO/plane"
        );
        assert_eq!(
            redact_local_paths("/tmp/Development/crucible-evals/evals/x.json"),
            "$CRUCIBLE_EVALS_REPO/evals/x.json"
        );
    }

    fn fresh_tmp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "threshold-optimization-target-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    struct TrialRow<'a> {
        candidate_id: &'a str,
        candidate_kind: &'a str,
        task_id: &'a str,
        expected: u64,
        matched: &'a [&'a str],
        false_positives: u64,
        reward: f64,
        cost: Option<f64>,
    }

    impl<'a> TrialRow<'a> {
        fn new(
            candidate_id: &'a str,
            candidate_kind: &'a str,
            task_id: &'a str,
            expected: u64,
            matched: &'a [&'a str],
        ) -> Self {
            TrialRow {
                candidate_id,
                candidate_kind,
                task_id,
                expected,
                matched,
                false_positives: 0,
                reward: if expected == matched.len() as u64 {
                    1.0
                } else {
                    0.0
                },
                cost: None,
            }
        }

        fn false_positives(mut self, false_positives: u64) -> Self {
            self.false_positives = false_positives;
            self
        }

        fn reward(mut self, reward: f64) -> Self {
            self.reward = reward;
            self
        }

        fn cost(mut self, cost: f64) -> Self {
            self.cost = Some(cost);
            self
        }

        fn to_jsonl(&self) -> String {
            serde_json::to_string(&json!({
                "run_id": format!("{}-{}", self.candidate_id, self.task_id),
                "candidate_id": self.candidate_id,
                "candidate_kind": self.candidate_kind,
                "composition_hash": format!("{}-hash", self.candidate_id),
                "model": if self.candidate_kind == "oneshot" { Value::String("deepseek/deepseek-v4-pro".to_string()) } else { Value::Null },
                "task_id": self.task_id,
                "trial": 1,
                "cost_usd": self.cost,
                "wall_ms": 10,
                "reward": self.reward,
                "matched": self.matched,
                "false_positives": self.false_positives,
                "expected_defects": self.expected
            }))
            .unwrap()
        }
    }
}
