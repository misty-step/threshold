//! Seed a diverse agent population from the taskspec's declared search space.
//!
//! Port of `runner/seed.py`. The module is split into:
//!
//! - **Deterministic core** — `sample_compositions`, `build_seeds`, and the
//!   stance/packet-template constants. These are parity-tested in
//!   `tests/parity_seed.rs`.
//!
//! - **LLM boundary** — `author_packets` invokes an injected `call` closure
//!   (same pattern as `mutate.rs`). The closure is NOT parity-tested; a fake
//!   is injected in unit tests.
//!
//! - **Top-level orchestration** — `seed_population` wires together the
//!   three sub-steps. The non-deterministic default-seed path
//!   (`random.randrange(2**32)`) is production-only and is not tested here.
//!
//! ## RNG contract
//!
//! Python uses `random.Random(seed)` (CPython MT19937). Rust uses
//! `crate::pyrandom::PyRandom`, which is verified byte-for-bit against
//! CPython in `tests/parity_pyrandom.rs`. Keep `rng` injected so parity
//! tests can compare shuffle trajectories directly.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::prompt_packet::is_sane_prompt_packet;
use crate::pyrandom::PyRandom;

/// Return type for `author_packets`: list of `(name, path)` pairs and costs.
type PacketResult = Result<(Vec<(String, PathBuf)>, Vec<f64>), String>;

/// Return type for `seed_population`: list of `(seed_id, path)` pairs and meta.
type SeedResult = Result<(Vec<(String, PathBuf)>, Map<String, Value>), String>;

// ---------------------------------------------------------------------------
// Module-level constants (mirrors Python module globals)
// ---------------------------------------------------------------------------

/// Default tool-policy map when the search space omits `tool_policies`.
pub const DEFAULT_POLICIES: &[(&str, &[&str])] = &[("full", &["read", "bash", "edit", "write"])];

/// Stance library for packet diversity.
/// Each `(name, brief)` pair matches `seed.py:STANCES` exactly (including
/// punctuation and trailing sentence), so the Python parity driver can
/// cross-check by sending the same brief string through `PACKET_BRIEF`.
pub const STANCES: &[(&str, &str)] = &[
    (
        "checklist",
        "Systematic checklist review: enumerate the defect \
taxonomy and check the change against every category in order.",
    ),
    (
        "skeptic",
        "Minimal-false-positive review: report only findings you \
can prove from the code in front of you; when unsure, stay silent.",
    ),
    (
        "spec-first",
        "Specification-first review: read SPEC/docs/invariants \
before the diff; flag violations of documented contracts.",
    ),
    (
        "trace-callers",
        "Cross-file dataflow review: for every changed \
function, trace its callers and callees before judging the change.",
    ),
    (
        "test-runner",
        "Evidence-by-execution review: when tests or a runnable \
entrypoint exist, run them and ground findings in observed behavior.",
    ),
];

/// Template used to build the prompt sent to the optimizer for each stance.
pub const PACKET_BRIEF: &str = "\
Write a system prompt (a \"prompt packet\") for a focused review agent.\n\
\n\
Task goal: {goal}\n\
Review stance the packet must embody: {stance}\n\
\n\
Requirements: under 250 words, imperative voice, no preamble, no markdown\n\
headers. The packet must instruct the agent to ground every finding in\n\
file/line evidence and to report nothing on a clean change.\n\
Respond with ONLY the packet text.";

// ---------------------------------------------------------------------------
// sample_compositions  (deterministic core — parity-tested)
// ---------------------------------------------------------------------------

/// A single slot combination produced by `sample_compositions`.
#[derive(Debug, Clone, PartialEq)]
pub struct Composition {
    pub model: String,
    pub thinking: String,
    pub policy_name: String,
    pub tools: Vec<String>,
    pub system_prompt_mode: String,
    pub skill_set_name: Option<String>,
    pub skills: Option<Vec<String>>,
    pub agents_md: Option<String>,
}

/// `n` slot combos cycling each shuffled axis independently.
///
/// Mirrors `seed.py::sample_compositions` exactly:
/// - `models` and `levels` are shuffled from their list form.
/// - `policies` and `skill_sets` are sorted by key first, then shuffled
///   (Python `sorted(dict.items())`).
/// - Optional axes (`system_prompt_modes`, `skill_sets`, `agents_md_options`)
///   default to `["append"]`, `[(None, None)]`, and `[None]` when absent.
///
/// The `rng` is consumed (advanced) by the shuffles, matching Python semantics.
pub fn sample_compositions(
    search: &Map<String, Value>,
    n: usize,
    rng: &mut PyRandom,
) -> Vec<Composition> {
    // --- models ---
    let mut models: Vec<String> = search
        .get("models")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    rng.shuffle(&mut models);

    // --- thinking_levels ---
    let mut levels: Vec<String> = search
        .get("thinking_levels")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["medium".to_owned()]);
    rng.shuffle(&mut levels);

    // --- tool_policies: sorted(dict.items()) then shuffled ---
    let raw_policies = search.get("tool_policies").and_then(Value::as_object);
    let mut policies: Vec<(String, Vec<String>)> = if let Some(obj) = raw_policies {
        let mut v: Vec<(String, Vec<String>)> = obj
            .iter()
            .map(|(k, v)| {
                let tools = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                (k.clone(), tools)
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    } else {
        DEFAULT_POLICIES
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    };
    rng.shuffle(&mut policies);

    // --- system_prompt_modes ---
    let mut sp_modes: Vec<String> = search
        .get("system_prompt_modes")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["append".to_owned()]);
    rng.shuffle(&mut sp_modes);

    // --- skill_sets: sorted(dict.items()) or [(None, None)] ---
    // Python: sorted((search.get("skill_sets") or {}).items()) or [(None, None)]
    // The `or [(None, None)]` applies when the sorted result is empty (i.e. dict
    // is absent or empty).
    let raw_skill_sets = search.get("skill_sets").and_then(Value::as_object);
    let mut skill_sets: Vec<(Option<String>, Option<Vec<String>>)> =
        if let Some(obj) = raw_skill_sets {
            let mut v: Vec<(Option<String>, Option<Vec<String>>)> = obj
                .iter()
                .map(|(k, v)| {
                    let files: Option<Vec<String>> = v.as_array().map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    });
                    (Some(k.clone()), files)
                })
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            if v.is_empty() {
                vec![(None, None)]
            } else {
                v
            }
        } else {
            vec![(None, None)]
        };
    rng.shuffle(&mut skill_sets);

    // --- agents_md_options ---
    let mut agents_opts: Vec<Option<String>> = search
        .get("agents_md_options")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.iter()
                .map(|v| {
                    if v.is_null() {
                        None
                    } else {
                        v.as_str().map(str::to_owned)
                    }
                })
                .collect()
        })
        .unwrap_or_else(|| vec![None]);
    rng.shuffle(&mut agents_opts);

    // --- build combos ---
    let mut combos = Vec::with_capacity(n);
    for i in 0..n {
        let (set_name, set_files) = skill_sets[i % skill_sets.len()].clone();
        combos.push(Composition {
            model: models[i % models.len()].clone(),
            thinking: levels[i % levels.len()].clone(),
            policy_name: policies[i % policies.len()].0.clone(),
            tools: policies[i % policies.len()].1.clone(),
            system_prompt_mode: sp_modes[i % sp_modes.len()].clone(),
            skill_set_name: set_name,
            // Python: list(set_files) if set_files else None
            // In Python, an empty list is falsy, so an empty skill_set → None.
            skills: set_files.filter(|f| !f.is_empty()),
            agents_md: agents_opts[i % agents_opts.len()].clone(),
        });
    }
    combos
}

// ---------------------------------------------------------------------------
// author_packets  (LLM boundary — not parity-tested; fake `call` in tests)
// ---------------------------------------------------------------------------

/// Write `k` stance packets authored by the optimizer, falling back to
/// `fallback_text` when the call fails or returns degenerate text.
///
/// `call` signature: `(prompt: &str, model: &str) -> Result<(String, f64), E>`
/// Returns `(Vec<(name, path)>, Vec<f64>)` mirroring Python.
pub fn author_packets<F, E>(
    taskspec: &Map<String, Value>,
    k: usize,
    optimizer_model: &str,
    rng: &mut PyRandom,
    packets_dir: &Path,
    call: &mut F,
    fallback_text: Option<&str>,
) -> PacketResult
where
    F: FnMut(&str, &str) -> Result<(String, f64), E>,
    E: std::fmt::Display,
{
    let mut stances: Vec<(&str, &str)> = STANCES.to_vec();
    rng.shuffle(&mut stances);
    std::fs::create_dir_all(packets_dir)
        .map_err(|e| format!("create {}: {e}", packets_dir.display()))?;

    let goal = taskspec.get("goal").and_then(Value::as_str).unwrap_or("");
    let mut packets: Vec<(String, PathBuf)> = Vec::new();
    let mut costs: Vec<f64> = Vec::new();

    for (name, brief) in stances.iter().take(k) {
        let prompt = PACKET_BRIEF
            .replace("{goal}", goal)
            .replace("{stance}", brief);
        let (text, used_fallback) = match call(&prompt, optimizer_model) {
            Err(e) => {
                // exception → fallback or re-raise
                if let Some(fb) = fallback_text {
                    (fb.to_owned(), true)
                } else {
                    return Err(format!("optimizer call failed: {e}"));
                }
            }
            Ok((mut text, cost)) => {
                costs.push(cost);
                text = text.trim().to_owned() + "\n";
                // sanity check
                if is_sane_prompt_packet(&text) {
                    (text, false)
                } else if let Some(fb) = fallback_text {
                    (fb.to_owned(), true)
                } else {
                    return Err("optimizer returned degenerate packet text".to_owned());
                }
            }
        };
        let _ = used_fallback; // explicit to mirror the Python control flow shape
        let path = packets_dir.join(format!("seed-{name}.md"));
        std::fs::write(&path, &text).map_err(|e| format!("write {}: {e}", path.display()))?;
        packets.push((name.to_string(), path));
    }
    Ok((packets, costs))
}

// ---------------------------------------------------------------------------
// build_seeds  (deterministic core — parity-tested via seed_population)
// ---------------------------------------------------------------------------

/// Materialize one hashed pi manifest per combo, packets round-robin.
///
/// `temperature`/`max_tokens` are deliberately absent: pi has no flag for them.
///
/// Returns `Vec<(seed_id, manifest_path)>`.
pub fn build_seeds(
    combos: &[Composition],
    packets: &[(String, PathBuf)],
    manifests_dir: &Path,
    timeout_sec: i64,
) -> Result<Vec<(String, PathBuf)>, String> {
    std::fs::create_dir_all(manifests_dir)
        .map_err(|e| format!("create {}: {e}", manifests_dir.display()))?;
    let mut out = Vec::new();
    for (i, combo) in combos.iter().enumerate() {
        let (pname, ppath) = &packets[i % packets.len()];
        let model_slug = combo
            .model
            .split('/')
            .next_back()
            .unwrap_or(&combo.model)
            .replace('.', "-");
        let raw_id = format!("seed{}-{}-{}", i + 1, model_slug, pname);
        let seed_id: String = raw_id.chars().take(48).collect();

        // Build the manifest map in Python insertion order.
        let mut manifest: Map<String, Value> = Map::new();
        manifest.insert("composition".into(), Value::Number(1.into()));
        manifest.insert("id".into(), Value::String(seed_id.clone()));
        manifest.insert("kind".into(), Value::String("pi".into()));
        manifest.insert("provider_name".into(), Value::String("openrouter".into()));
        manifest.insert("model".into(), Value::String(combo.model.clone()));
        manifest.insert(
            "prompt_packet".into(),
            Value::String(ppath.to_string_lossy().into_owned()),
        );
        manifest.insert("thinking".into(), Value::String(combo.thinking.clone()));
        manifest.insert(
            "tools".into(),
            Value::Array(
                combo
                    .tools
                    .iter()
                    .map(|t| Value::String(t.clone()))
                    .collect(),
            ),
        );
        manifest.insert("timeout_sec".into(), Value::Number(timeout_sec.into()));

        // Optional fields — only written when non-default.
        // Python: `if combo.get("system_prompt_mode", "append") != "append"`
        if combo.system_prompt_mode != "append" {
            manifest.insert(
                "system_prompt_mode".into(),
                Value::String(combo.system_prompt_mode.clone()),
            );
        }
        // Python: `if combo.get("skills")`
        if let Some(skills) = &combo.skills {
            manifest.insert(
                "skills".into(),
                Value::Array(skills.iter().map(|s| Value::String(s.clone())).collect()),
            );
        }
        // Python: `if combo.get("agents_md")`
        if let Some(agents_md) = &combo.agents_md {
            manifest.insert("agents_md".into(), Value::String(agents_md.clone()));
        }

        let path = manifests_dir.join(format!("{seed_id}.toml"));
        crate::mutate::write_manifest(&manifest, &path)
            .map_err(|e| format!("write manifest: {e}"))?;
        out.push((seed_id, path));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// seed_population  (orchestration — wires all three steps together)
// ---------------------------------------------------------------------------

/// Full step: search space → combos → packets → manifests on disk.
///
/// Returns `(Vec<(seed_id, manifest_path)>, meta_map)` where `meta_map`
/// matches the Python `meta` dict structure for downstream lineage tracking.
///
/// The non-deterministic default-seed path (`random.randrange(2**32)`)
/// is production-only; pass `rng_seed=None` to trigger it. Parity tests
/// always supply an explicit seed.
pub fn seed_population<F, E>(
    spec: &Map<String, Value>,
    optimizer_model: &str,
    packets_dir: &Path,
    manifests_dir: &Path,
    rng_seed: Option<u64>,
    repo_root: Option<&Path>,
    call: &mut F,
) -> SeedResult
where
    F: FnMut(&str, &str) -> Result<(String, f64), E>,
    E: std::fmt::Display,
{
    let search = spec
        .get("search")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    // models is required
    let has_models = search
        .get("models")
        .and_then(Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if !has_models {
        return Err("taskspec [search] must declare a models list".to_owned());
    }

    let actual_seed = rng_seed.unwrap_or_else(|| {
        // non-deterministic production path
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
            .unwrap_or(0)
    });
    let mut rng = PyRandom::new(actual_seed);

    let n = search
        .get("seed_count")
        .and_then(Value::as_i64)
        .map(|v| v as usize)
        .unwrap_or(6);

    let combos = sample_compositions(&search, n, &mut rng);

    // base_packet fallback
    let base_ref = search.get("base_packet").and_then(Value::as_str);
    let fallback_text_owned: Option<String> = base_ref.and_then(|r| {
        let p = repo_root
            .map(|root| root.join(r))
            .unwrap_or_else(|| PathBuf::from(r));
        std::fs::read_to_string(&p).ok()
    });
    let fallback_text = fallback_text_owned.as_deref();

    let k = search
        .get("packet_stances")
        .and_then(Value::as_i64)
        .map(|v| v as usize)
        .unwrap_or_else(|| n.min(3));

    let (packets, costs) = author_packets(
        spec,
        k,
        optimizer_model,
        &mut rng,
        packets_dir,
        call,
        fallback_text,
    )?;

    let timeout = spec
        .get("budget")
        .and_then(Value::as_object)
        .and_then(|b| b.get("max_wall_per_trial_sec"))
        .and_then(Value::as_i64)
        .unwrap_or(600);

    let seeds = build_seeds(&combos, &packets, manifests_dir, timeout)?;

    // Build meta dict matching Python output structure.
    let mut meta: Map<String, Value> = Map::new();
    meta.insert("rng_seed".into(), Value::Number(actual_seed.into()));
    meta.insert("seed_count".into(), Value::Number(n.into()));
    meta.insert(
        "packet_stances".into(),
        Value::Array(
            packets
                .iter()
                .map(|(nm, _)| Value::String(nm.clone()))
                .collect(),
        ),
    );
    meta.insert(
        "optimizer_costs".into(),
        Value::Array(
            costs
                .iter()
                .map(|&c| {
                    serde_json::Number::from_f64(c)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                })
                .collect(),
        ),
    );
    // combos omit "tools" and "skills" keys, matching Python:
    //   {k_: v for k_, v in c.items() if k_ not in ("tools", "skills")}
    meta.insert(
        "combos".into(),
        Value::Array(
            combos
                .iter()
                .map(|c| {
                    let mut m = Map::new();
                    m.insert("model".into(), Value::String(c.model.clone()));
                    m.insert("thinking".into(), Value::String(c.thinking.clone()));
                    m.insert("policy_name".into(), Value::String(c.policy_name.clone()));
                    m.insert(
                        "system_prompt_mode".into(),
                        Value::String(c.system_prompt_mode.clone()),
                    );
                    m.insert(
                        "skill_set_name".into(),
                        c.skill_set_name
                            .as_ref()
                            .map(|s| Value::String(s.clone()))
                            .unwrap_or(Value::Null),
                    );
                    m.insert(
                        "agents_md".into(),
                        c.agents_md
                            .as_ref()
                            .map(|s| Value::String(s.clone()))
                            .unwrap_or(Value::Null),
                    );
                    Value::Object(m)
                })
                .collect(),
        ),
    );

    Ok((seeds, meta))
}

// ---------------------------------------------------------------------------
// Unit tests (port of tests/test_seed.py; fake `call` injected)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn search_map() -> Map<String, Value> {
        json!({
            "models": [
                "deepseek/deepseek-v4-flash",
                "z-ai/glm-4.7-flash",
                "openai/gpt-5-mini",
                "moonshotai/kimi-k2.6",
                "z-ai/glm-5",
                "qwen/qwen3.7-plus"
            ],
            "thinking_levels": ["off", "low", "medium", "high"],
            "tool_policies": {
                "full": ["read", "bash", "edit", "write"],
                "explore": ["read", "bash"],
                "no-exec": ["read", "edit", "write"]
            },
            "packet_stances": 3,
            "seed_count": 6
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn spec_map() -> Map<String, Value> {
        json!({
            "goal": "find the real defects",
            "search": search_map_value(),
            "budget": {"max_wall_per_trial_sec": 300}
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn search_map_value() -> Value {
        json!({
            "models": [
                "deepseek/deepseek-v4-flash",
                "z-ai/glm-4.7-flash",
                "openai/gpt-5-mini",
                "moonshotai/kimi-k2.6",
                "z-ai/glm-5",
                "qwen/qwen3.7-plus"
            ],
            "thinking_levels": ["off", "low", "medium", "high"],
            "tool_policies": {
                "full": ["read", "bash", "edit", "write"],
                "explore": ["read", "bash"],
                "no-exec": ["read", "edit", "write"]
            },
            "packet_stances": 3,
            "seed_count": 6
        })
    }

    /// Mirror of `tests/test_seed.py::fake_call`
    fn fake_call(prompt: &str, _model: &str) -> Result<(String, f64), String> {
        let stance = STANCES
            .iter()
            .find(|(_, brief)| prompt.contains(brief))
            .map(|(name, _)| *name)
            .unwrap_or("generic");
        Ok((
            format!(
                "You are a {} reviewer. Ground every finding in evidence.",
                stance
            ),
            0.001,
        ))
    }

    #[test]
    fn sampling_spans_the_axes_and_is_deterministic() {
        let search = search_map();
        let a = sample_compositions(&search, 6, &mut PyRandom::new(7));
        let b = sample_compositions(&search, 6, &mut PyRandom::new(7));
        assert_eq!(a, b, "same seed must give same compositions");

        let models_a: std::collections::HashSet<&str> =
            a.iter().map(|c| c.model.as_str()).collect();
        assert_eq!(models_a.len(), 6, "all six models should be distinct");

        let levels_a: std::collections::HashSet<&str> =
            a.iter().map(|c| c.thinking.as_str()).collect();
        assert!(
            levels_a.len() >= 3,
            "at least 3 thinking levels should appear"
        );

        let policies_a: std::collections::HashSet<&str> =
            a.iter().map(|c| c.policy_name.as_str()).collect();
        assert_eq!(policies_a.len(), 3, "all three policies should appear");

        let different = sample_compositions(&search, 6, &mut PyRandom::new(8));
        assert_ne!(
            different, a,
            "different seed should give different compositions"
        );
    }

    #[test]
    fn seed_population_materializes_hashed_pi_manifests() {
        let tmp = std::env::temp_dir().join(format!("threshold-seed-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let packets_dir = tmp.join("packets");
        let manifests_dir = tmp.join("manifests");

        let spec = spec_map();
        let mut call = |prompt: &str, model: &str| fake_call(prompt, model);
        let (seeds, meta) = seed_population(
            &spec,
            "opt-model",
            &packets_dir,
            &manifests_dir,
            Some(42),
            None,
            &mut call,
        )
        .unwrap();

        assert_eq!(seeds.len(), 6);
        assert_eq!(meta["rng_seed"], json!(42u64));
        let stances = meta["packet_stances"].as_array().unwrap();
        assert_eq!(stances.len(), 3);
        let costs = meta["optimizer_costs"].as_array().unwrap();
        assert_eq!(costs, &[json!(0.001), json!(0.001), json!(0.001)]);

        let ids: Vec<&str> = seeds.iter().map(|(id, _)| id.as_str()).collect();
        let unique_ids: std::collections::HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique_ids.len(), 6, "seed ids must be distinct");

        let all_thinking: Vec<String> = ["off", "low", "medium", "high"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        for (_, path) in &seeds {
            let content = std::fs::read_to_string(path).unwrap();
            // Parse TOML manually (no toml crate dep check needed — it's available)
            let manifest: toml::Value = toml::from_str(&content).unwrap();
            assert_eq!(manifest["kind"].as_str().unwrap(), "pi");
            let model = manifest["model"].as_str().unwrap();
            let search_models = &[
                "deepseek/deepseek-v4-flash",
                "z-ai/glm-4.7-flash",
                "openai/gpt-5-mini",
                "moonshotai/kimi-k2.6",
                "z-ai/glm-5",
                "qwen/qwen3.7-plus",
            ];
            assert!(
                search_models.contains(&model),
                "model {model} not in search space"
            );
            let thinking = manifest["thinking"].as_str().unwrap();
            assert!(
                all_thinking.contains(&thinking.to_string()),
                "thinking {thinking} not valid"
            );
            // tools is a JSON array stored as a string in the TOML
            assert!(manifest.get("timeout_sec").is_some());
            assert_eq!(manifest["timeout_sec"].as_integer().unwrap(), 300);
            // pi must not have temperature or max_tokens
            assert!(
                manifest.get("temperature").is_none(),
                "temperature must not be in seed manifest"
            );
            assert!(
                manifest.get("max_tokens").is_none(),
                "max_tokens must not be in seed manifest"
            );
            // prompt_packet must point to a readable file with content
            let pp = manifest["prompt_packet"].as_str().unwrap();
            let pp_text = std::fs::read_to_string(pp).unwrap();
            assert!(
                !pp_text.trim().is_empty(),
                "prompt_packet file must have content"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn packet_author_failure_falls_back_to_base() {
        let tmp =
            std::env::temp_dir().join(format!("threshold-seed-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let spec = spec_map();
        let base_text = "Base reviewer packet.\n";

        // Without fallback → propagates error
        let mut broken_call = |_p: &str, _m: &str| -> Result<(String, f64), String> {
            Err("optimizer down".to_owned())
        };
        let result = author_packets(
            &spec,
            2,
            "m",
            &mut PyRandom::new(1),
            &tmp.join("p1"),
            &mut broken_call,
            None,
        );
        assert!(result.is_err(), "should propagate error when no fallback");

        // With fallback → all packets use fallback text; no costs recorded.
        let mut broken_call2 = |_p: &str, _m: &str| -> Result<(String, f64), String> {
            Err("optimizer down".to_owned())
        };
        let (packets, costs) = author_packets(
            &spec,
            2,
            "m",
            &mut PyRandom::new(1),
            &tmp.join("p2"),
            &mut broken_call2,
            Some(base_text),
        )
        .unwrap();
        assert_eq!(packets.len(), 2);
        assert!(costs.is_empty(), "no cost when call fails");
        for (_, path) in &packets {
            assert_eq!(
                std::fs::read_to_string(path).unwrap(),
                base_text,
                "fallback text must be written verbatim"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn packet_author_degenerate_text_falls_back_to_base() {
        let tmp = std::env::temp_dir().join(format!("threshold-seed-degen-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let fallback = "You are a precise code-review agent. Ground every finding in file \
and line evidence. Report zero findings on a clean change.\n";
        let spec = spec_map();

        // Without fallback → error on degenerate text
        let mut degen_call = |_p: &str, _m: &str| -> Result<(String, f64), String> {
            Ok(("The".to_owned() + &"!".repeat(5000), 0.123))
        };
        let result = author_packets(
            &spec,
            1,
            "m",
            &mut PyRandom::new(1),
            &tmp.join("p1"),
            &mut degen_call,
            None,
        );
        assert!(
            result.is_err(),
            "should error on degenerate text without fallback"
        );

        // With fallback → costs recorded (call succeeded), packet is fallback
        let mut degen_call2 = |_p: &str, _m: &str| -> Result<(String, f64), String> {
            Ok(("The".to_owned() + &"!".repeat(5000), 0.123))
        };
        let (packets, costs) = author_packets(
            &spec,
            1,
            "m",
            &mut PyRandom::new(1),
            &tmp.join("p2"),
            &mut degen_call2,
            Some(fallback),
        )
        .unwrap();
        assert_eq!(
            costs,
            vec![0.123],
            "cost recorded even when text was degenerate"
        );
        assert_eq!(packets.len(), 1);
        assert_eq!(std::fs::read_to_string(&packets[0].1).unwrap(), fallback);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn optional_axes_sampled_when_declared() {
        let tmp =
            std::env::temp_dir().join(format!("threshold-seed-optional-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let skill = tmp.join("skill.md");
        std::fs::write(&skill, "# skill").unwrap();
        let agents = tmp.join("agents.md");
        std::fs::write(&agents, "workspace briefing for the review agent").unwrap();

        let search = json!({
            "models": [
                "deepseek/deepseek-v4-flash",
                "z-ai/glm-4.7-flash",
                "openai/gpt-5-mini",
                "moonshotai/kimi-k2.6",
                "z-ai/glm-5",
                "qwen/qwen3.7-plus"
            ],
            "thinking_levels": ["off", "low", "medium", "high"],
            "tool_policies": {
                "full": ["read", "bash", "edit", "write"],
                "explore": ["read", "bash"],
                "no-exec": ["read", "edit", "write"]
            },
            "packet_stances": 3,
            "seed_count": 6,
            "system_prompt_modes": ["append", "replace"],
            "skill_sets": {
                "pack": [skill.to_str().unwrap()],
                "bare": []
            },
            "agents_md_options": [agents.to_str().unwrap()]
        });
        let mut spec_obj = spec_map();
        spec_obj.insert("search".into(), search);

        let mut call = |prompt: &str, model: &str| fake_call(prompt, model);
        let (seeds, meta) = seed_population(
            &spec_obj,
            "opt",
            &tmp.join("p"),
            &tmp.join("m"),
            Some(3),
            None,
            &mut call,
        )
        .unwrap();

        // At least one manifest should have system_prompt_mode = replace
        let mut has_replace = false;
        let mut has_skill = false;
        let mut all_have_agents_md = true;
        let agents_str = agents.to_str().unwrap();
        let skill_str = skill.to_str().unwrap();

        for (_, path) in &seeds {
            let content = std::fs::read_to_string(path).unwrap();
            let m: toml::Value = toml::from_str(&content).unwrap();
            if m.get("system_prompt_mode").and_then(|v| v.as_str()) == Some("replace") {
                has_replace = true;
            }
            if let Some(skills_val) = m.get("skills") {
                // skills is a TOML array of strings (written via write_manifest's JSON array path)
                let contains_skill = skills_val
                    .as_array()
                    .map(|a| a.iter().any(|v| v.as_str() == Some(skill_str)))
                    .unwrap_or_else(|| {
                        // fallback: check string representation
                        skills_val.as_str().unwrap_or("").contains(skill_str)
                    });
                if contains_skill {
                    has_skill = true;
                }
            }
            match m.get("agents_md").and_then(|v| v.as_str()) {
                Some(v) if v == agents_str => {}
                _ => {
                    all_have_agents_md = false;
                }
            }
        }
        assert!(
            has_replace,
            "at least one manifest should have system_prompt_mode=replace"
        );
        assert!(
            has_skill,
            "at least one manifest should have the skill file"
        );
        assert!(all_have_agents_md, "all manifests should have agents_md");

        // meta combos have system_prompt_mode but not skills
        let combos = meta["combos"].as_array().unwrap();
        assert!(
            combos.iter().all(|c| c.get("system_prompt_mode").is_some()),
            "combos must record system_prompt_mode"
        );
        assert!(
            combos.iter().all(|c| c.get("skills").is_none()),
            "combos must not record skills"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_models_raises() {
        let tmp =
            std::env::temp_dir().join(format!("threshold-seed-nomodel-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let spec = json!({"search": {}}).as_object().unwrap().clone();
        let mut call =
            |_p: &str, _m: &str| -> Result<(String, f64), String> { Ok(("ok".into(), 0.0)) };
        let result = seed_population(&spec, "m", &tmp, &tmp, Some(1), None, &mut call);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("models"),
            "error should mention models"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
