//! Deterministic core of `runner/run.py`, ported to Rust.
//!
//! **Scope** — pure, deterministic helpers only. Live-I/O glue
//! (`run_oneshot`, `run_pi`, `harness_version`, `main`) is lead-owned and not
//! included here. The boundary is the same as the Python module: these
//! functions are callable without a network, subprocess, or temp-dir side
//! effect.
//!
//! Every public function is parity-tested in `tests/parity_run.rs` against the
//! Python original. See `docs/rust-migration.md` for migration status.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::pycompat::{py_json_dumps, round_half_even};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Raised when an arena requires isolation stronger than temp dirs.
#[derive(Debug)]
pub struct LocalExecutionRefused(pub String);

impl std::fmt::Display for LocalExecutionRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for LocalExecutionRefused {}

/// Raised when candidate-visible text exposes hidden grader paths.
#[derive(Debug)]
pub struct GraderPathLeak(pub String);

impl std::fmt::Display for GraderPathLeak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for GraderPathLeak {}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const LOCAL_SAFE_RISK_CLASSES: &[&str] = &["low"];

/// Maps TOML risk flag keys → human-readable labels.
const LOCAL_REFUSAL_FLAGS: &[(&str, &str)] = &[
    ("needs_network", "network"),
    ("needs_secrets", "secrets"),
    ("adversarial_fixtures", "adversarial fixtures"),
    ("user_data", "user data"),
];

// ---------------------------------------------------------------------------
// tree_digest
// ---------------------------------------------------------------------------

/// Stable hash over file paths + contents, for grader tamper detection.
///
/// Mirrors:
/// ```python
/// h = hashlib.sha256()
/// for root in roots:
///     for f in sorted(Path(root).rglob("*")):
///         if f.is_file():
///             h.update(str(f.relative_to(root)).encode())
///             h.update(f.read_bytes())
/// return h.hexdigest()
/// ```
///
/// Python's `Path.rglob("*")` returns all entries; `sorted()` sorts by their
/// string representation lexicographically. Only files (not directories)
/// contribute to the hash — same as Python's `if f.is_file()` guard.
pub fn tree_digest(roots: &[&Path]) -> String {
    let mut hasher = Sha256::new();
    for &root in roots {
        // Collect all entries recursively, then sort by relative path string.
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        collect_files(root, root, &mut files);
        // Python sorts Path objects lexicographically (by their string form).
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (rel_str, abs_path) in &files {
            if abs_path.is_file() {
                hasher.update(rel_str.as_bytes());
                if let Ok(bytes) = std::fs::read(abs_path) {
                    hasher.update(&bytes);
                }
            }
        }
    }
    format!("{:x}", hasher.finalize())
}

/// Recursively walk `dir`, collecting (relative_path_str, absolute_path) for
/// every entry (files and dirs alike — the `is_file()` filter is in the caller,
/// matching Python's `if f.is_file()` guard). Uses forward slashes on all
/// platforms so the sort order matches Python's `str(f.relative_to(root))`.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Build the relative path string exactly as Python does:
        // `str(f.relative_to(root))` uses forward slashes on POSIX.
        let rel = path
            .strip_prefix(root)
            .expect("path under root")
            .to_string_lossy()
            .replace('\\', "/");
        out.push((rel, path.clone()));
        if path.is_dir() {
            collect_files(root, &path, out);
        }
    }
}

// ---------------------------------------------------------------------------
// validate_task_dir
// ---------------------------------------------------------------------------

/// Reject fixture trees that could leak paths outside the task directory.
pub fn validate_task_dir(task_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    validate_task_dir_inner(task_dir)
}

fn validate_task_dir_inner(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            return Err(format!("fixture contains symlink: {}", path.display()).into());
        }
        if path.is_dir() {
            validate_task_dir_inner(&path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// validate_arena_for_local_execution
// ---------------------------------------------------------------------------

/// Refuse arena classes that must run behind Harbor/Docker isolation.
///
/// Mirrors Python's `validate_arena_for_local_execution(arena)` exactly,
/// including the error message strings.
pub fn validate_arena_for_local_execution(arena: &Value) -> Result<(), LocalExecutionRefused> {
    let risk = match arena.get("risk") {
        Some(r) => r,
        None => {
            return Err(LocalExecutionRefused(
                "arena risk requires Harbor/Docker isolation; local temp-dir \
runner refused (missing [risk] metadata). Add reviewed low-risk \
metadata or use bin/harbor-run for isolated execution."
                    .to_string(),
            ));
        }
    };
    let risk_class = risk
        .get("class")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();

    let mut risky_flags: Vec<String> = Vec::new();
    for &(key, label) in LOCAL_REFUSAL_FLAGS {
        if py_is_truthy_json(risk.get(key)) {
            risky_flags.push(label.to_string());
        }
    }
    if !LOCAL_SAFE_RISK_CLASSES.contains(&risk_class.as_str()) {
        risky_flags.insert(0, format!("class={risk_class}"));
    }
    if !risky_flags.is_empty() {
        // dict.fromkeys(risky_flags) deduplicates while preserving insertion order.
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<&String> = risky_flags
            .iter()
            .filter(|f| seen.insert(f.as_str()))
            .collect();
        let detail = deduped
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(LocalExecutionRefused(format!(
            "arena risk requires Harbor/Docker isolation; local temp-dir \
runner refused ({detail}). Use bin/harbor-run for isolated \
execution, or lower the arena risk metadata only after review."
        )));
    }
    Ok(())
}

/// Python `bool(risk.get(key))` — Value::Null, Value::Bool(false), zero, empty
/// string/array/object are all falsy. Missing key is None → falsy.
fn py_is_truthy_json(v: Option<&Value>) -> bool {
    match v {
        None => false,
        Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// _read_visible_texts
// ---------------------------------------------------------------------------

/// Iterator over (source_label, text) pairs that are visible to the candidate.
/// Mirrors Python's `_read_visible_texts(candidate, instruction, env_dir)`.
pub fn read_visible_texts<'a>(
    candidate: &'a Map<String, Value>,
    instruction: &'a str,
    env_dir: &'a Path,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    out.push(("instruction".to_string(), instruction.to_string()));

    if let Some(Value::String(text)) = candidate.get("_packet_text") {
        if !text.is_empty() {
            out.push(("prompt_packet".to_string(), text.clone()));
        }
    }
    if let Some(Value::String(text)) = candidate.get("_agents_md_text") {
        out.push(("agents_md".to_string(), text.clone()));
    }
    if let Some(Value::Array(skills)) = candidate.get("_skills_texts") {
        for (i, skill) in skills.iter().enumerate() {
            if let Value::String(text) = skill {
                out.push((format!("skill[{}]", i + 1), text.clone()));
            }
        }
    }

    // sorted(Path(env_dir).rglob("*")) — files only, text files only
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(env_dir, env_dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    for (rel_str, abs_path) in files {
        if !abs_path.is_file() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&abs_path) {
            out.push((rel_str, text));
        }
        // Non-UTF-8 files are silently skipped (UnicodeDecodeError in Python)
    }

    out
}

// ---------------------------------------------------------------------------
// validate_no_hidden_absolute_paths
// ---------------------------------------------------------------------------

/// Reject candidate-visible absolute paths into tests/ or solution/.
pub fn validate_no_hidden_absolute_paths(
    candidate: &Map<String, Value>,
    task_dir: &Path,
    instruction: &str,
    env_dir: &Path,
) -> Result<(), GraderPathLeak> {
    let hidden_roots = [
        task_dir
            .join("tests")
            .canonicalize()
            .unwrap_or_else(|_| task_dir.join("tests"))
            .to_string_lossy()
            .into_owned(),
        task_dir
            .join("solution")
            .canonicalize()
            .unwrap_or_else(|_| task_dir.join("solution"))
            .to_string_lossy()
            .into_owned(),
    ];
    for (source, text) in read_visible_texts(candidate, instruction, env_dir) {
        for root in &hidden_roots {
            if text.contains(root.as_str()) {
                return Err(GraderPathLeak(format!(
                    "candidate-visible {source} exposes hidden grader path: {root}"
                )));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// task_instruction
// ---------------------------------------------------------------------------

/// Compose the task instruction from the arena template + task intent, or fall
/// back to a per-task instruction.md for template-less arenas.
///
/// Mirrors Python:
/// ```python
/// template_ref = (arena.get("template") or {}).get("file")
/// if template_ref:
///     template = (arena_dir / template_ref).read_text()
///     intent = (task_dir / "intent.md").read_text().strip()
///     return template.replace("{intent}", intent)
/// return (task_dir / "instruction.md").read_text()
/// ```
pub fn task_instruction(
    arena_dir: &Path,
    arena: &Value,
    task_dir: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let template_ref = arena
        .get("template")
        .and_then(|t| t.get("file"))
        .and_then(Value::as_str);
    if let Some(tref) = template_ref {
        let template = std::fs::read_to_string(arena_dir.join(tref))?;
        let intent = std::fs::read_to_string(task_dir.join("intent.md"))?;
        let intent = intent.trim();
        return Ok(template.replace("{intent}", intent));
    }
    Ok(std::fs::read_to_string(task_dir.join("instruction.md"))?)
}

// ---------------------------------------------------------------------------
// select_tasks
// ---------------------------------------------------------------------------

/// Resolve task dirs honoring split selection and the holdout guard.
///
/// Mirrors Python's `select_tasks(arena_dir, arena, split, task_filter, final)`.
/// On a bad split, calls `sys.exit(...)` in Python; here we return `Err`.
pub fn select_tasks(
    arena_dir: &Path,
    arena: &Value,
    split: &str,
    task_filter: Option<&std::collections::HashSet<String>>,
    is_final: bool,
) -> Result<Vec<PathBuf>, String> {
    let split_cfg = arena.get("split").and_then(Value::as_object);
    let allowed: Option<std::collections::HashSet<String>> = if split != "all" {
        match split_cfg.and_then(|c| c.get(split)) {
            None => return Err(format!("arena declares no '{split}' split")),
            Some(Value::Array(ids)) => Some(
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
            ),
            _ => return Err(format!("arena declares no '{split}' split")),
        }
    } else {
        None
    };

    // sorted((arena_dir / "tasks").iterdir()) — directories only
    let tasks_dir = arena_dir.join("tasks");
    let mut task_dirs: Vec<PathBuf> = match std::fs::read_dir(&tasks_dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                allowed.as_ref().is_none_or(|a| a.contains(name))
                    && task_filter.is_none_or(|f| f.contains(name))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    task_dirs.sort();

    let holdout: std::collections::HashSet<String> = split_cfg
        .and_then(|c| c.get("holdout"))
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let mut touched_holdout: Vec<String> = task_dirs
        .iter()
        .filter_map(|d| {
            d.file_name()
                .and_then(|n| n.to_str())
                .filter(|n| holdout.contains(*n))
                .map(String::from)
        })
        .collect();
    touched_holdout.sort();

    if !touched_holdout.is_empty() && !is_final {
        return Err(format!(
            "holdout tasks require --final (anti-overfitting guard): {}",
            touched_holdout.join(", ")
        ));
    }
    Ok(task_dirs)
}

// ---------------------------------------------------------------------------
// load_toml
// ---------------------------------------------------------------------------

/// Load a TOML file and return it as a `serde_json::Value` (recursive
/// toml→json conversion). Mirrors Python's `load_toml(path)` return type
/// as used in the codebase: a plain dict.
pub fn load_toml(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let tv: toml::Value = toml::from_str(&text)?;
    Ok(toml_to_json(tv))
}

/// Backlog 040: a task's declared `source_repo` from its `task.toml` — the
/// cluster key that activates 039's repo-clustered statistics (tasks from the
/// same upstream repo share variance). `None` when the task is unlabeled or has
/// no `task.toml`, so callers fall back to per-task clustering.
pub fn source_repo(task_dir: &Path) -> Option<String> {
    let v = load_toml(&task_dir.join("task.toml")).ok()?;
    v.get("source_repo")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Recursively convert a `toml::Value` to a `serde_json::Value`.
/// Integer → Number(i64); Float → Number(f64); Boolean → Bool; String →
/// String; Array → Array; Table → Object; Datetime → String.
pub fn toml_to_json(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(serde_json::Number::from(i)),
        toml::Value::Float(f) => {
            Value::Number(serde_json::Number::from_f64(f).unwrap_or_else(|| 0.into()))
        }
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Array(arr) => Value::Array(arr.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => {
            let mut m: Map<String, Value> = Map::new();
            for (k, val) in t {
                m.insert(k, toml_to_json(val));
            }
            Value::Object(m)
        }
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
    }
}

// ---------------------------------------------------------------------------
// _resolve_ref
// ---------------------------------------------------------------------------

/// Resolve a relative or absolute file reference against the repo root.
/// Mirrors `_resolve_ref(ref)` from Python for live paths:
/// ```python
/// path = Path(ref)
/// return path if path.is_absolute() else REPO / path
/// ```
/// where `REPO` is the repository root (two levels above `runner/run.py`).
///
/// Legacy delivery manifests may contain absolute paths from the checkout that
/// produced the measured artifact. If that checkout path is gone, preserve the
/// manifest string for hashing but resolve known repo-local suffixes from the
/// current checkout so committed evidence remains portable.
pub fn resolve_ref(ref_str: &str, repo_root: &Path) -> PathBuf {
    let p = Path::new(ref_str);
    if p.is_absolute() {
        if p.exists() {
            return p.to_path_buf();
        }
        if let Some(path) = rebase_legacy_repo_ref(p, repo_root) {
            return path;
        }
        p.to_path_buf()
    } else {
        repo_root.join(p)
    }
}

fn rebase_legacy_repo_ref(path: &Path, repo_root: &Path) -> Option<PathBuf> {
    const REPO_ANCHORS: &[&str] = &[
        "approvals",
        "arenas",
        "backlog.d",
        "crates",
        "deliveries",
        "docs",
        "runs",
        "specs",
    ];

    let parts: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();

    for (index, part) in parts.iter().enumerate() {
        let Some(part) = part.to_str() else {
            continue;
        };
        if !REPO_ANCHORS.contains(&part) {
            continue;
        }
        let mut rebased = repo_root.to_path_buf();
        for suffix in &parts[index..] {
            rebased.push(suffix);
        }
        if rebased.exists() {
            return Some(rebased);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// load_candidate
// ---------------------------------------------------------------------------

/// Load a manifest, resolve its file-referenced slots (prompt packet, skills,
/// agents_md), compute the composition hash, and return the candidate map.
///
/// Private keys (underscore-prefixed) are included as `_packet_text`,
/// `_skill_paths`, `_skills_texts`, `_agents_md_text`, `_hash`.
///
/// The composition hash mirrors Python exactly:
/// ```python
/// basis = {k: v for k, v in candidate.items() if not k.startswith("_")}
/// basis["prompt_packet_text"] = candidate["_packet_text"]
/// basis["skills_texts"] = candidate["_skills_texts"]
/// basis["agents_md_text"] = candidate["_agents_md_text"]
/// candidate["_hash"] = hashlib.sha256(
///     json.dumps(basis, sort_keys=True).encode()
/// ).hexdigest()[:16]
/// ```
pub fn load_candidate(
    path: &Path,
    repo_root: &Path,
) -> Result<Map<String, Value>, Box<dyn std::error::Error>> {
    let raw = load_toml(path)?;
    let mut candidate = match raw {
        Value::Object(m) => m,
        _ => return Err("candidate manifest is not a TOML table".into()),
    };

    // Resolve prompt_packet
    let packet_text: Option<String> = match candidate.get("prompt_packet") {
        Some(Value::String(ref_str)) => {
            let p = resolve_ref(ref_str, repo_root);
            Some(std::fs::read_to_string(&p)?)
        }
        _ => None,
    };
    candidate.insert(
        "_packet_text".to_string(),
        match packet_text {
            Some(ref t) => Value::String(t.clone()),
            None => Value::Null,
        },
    );

    // Resolve skills
    let skill_paths: Vec<PathBuf> = match candidate.get("skills") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| resolve_ref(s, repo_root))
            .collect(),
        _ => Vec::new(),
    };
    candidate.insert(
        "_skill_paths".to_string(),
        Value::Array(
            skill_paths
                .iter()
                .map(|p| Value::String(p.to_string_lossy().into_owned()))
                .collect(),
        ),
    );
    let skills_texts: Vec<Value> = skill_paths
        .iter()
        .map(|p| std::fs::read_to_string(p).map(Value::String))
        .collect::<Result<_, _>>()?;
    candidate.insert("_skills_texts".to_string(), Value::Array(skills_texts));

    // Resolve agents_md
    let agents_md_text: Option<String> = match candidate.get("agents_md") {
        Some(Value::String(ref_str)) => {
            let p = resolve_ref(ref_str, repo_root);
            Some(std::fs::read_to_string(&p)?)
        }
        _ => None,
    };
    candidate.insert(
        "_agents_md_text".to_string(),
        match agents_md_text {
            Some(ref t) => Value::String(t.clone()),
            None => Value::Null,
        },
    );

    // Validate system_prompt_mode
    let mode = candidate
        .get("system_prompt_mode")
        .and_then(Value::as_str)
        .unwrap_or("append");
    if mode != "append" && mode != "replace" {
        return Err(format!("system_prompt_mode must be append|replace, got {mode:?}").into());
    }

    // Build basis for the composition hash: non-underscore keys + three
    // resolved texts.
    let mut basis: Map<String, Value> = Map::new();
    for (k, v) in &candidate {
        if !k.starts_with('_') {
            basis.insert(k.clone(), v.clone());
        }
    }
    // Python inserts these keys AFTER the manifest keys, in this exact order.
    basis.insert(
        "prompt_packet_text".to_string(),
        candidate["_packet_text"].clone(),
    );
    basis.insert(
        "skills_texts".to_string(),
        candidate["_skills_texts"].clone(),
    );
    basis.insert(
        "agents_md_text".to_string(),
        candidate["_agents_md_text"].clone(),
    );

    // sha256(json.dumps(basis, sort_keys=True).encode()).hexdigest()[:16]
    let dumped = py_json_dumps(&Value::Object(basis), true);
    let mut h = Sha256::new();
    h.update(dumped.as_bytes());
    let hex = format!("{:x}", h.finalize());
    let hash = hex[..16].to_string();
    candidate.insert("_hash".to_string(), Value::String(hash));

    Ok(candidate)
}

// ---------------------------------------------------------------------------
// workspace_listing
// ---------------------------------------------------------------------------

/// Build a workspace listing string for oneshot prompts.
///
/// Mirrors:
/// ```python
/// parts = []
/// for f in sorted(workdir.rglob("*")):
///     if f.is_file() and f.name != "findings.json":
///         rel = f.relative_to(workdir)
///         parts.append(f"\n### {rel}\n```\n{f.read_text()}```\n")
/// return "".join(parts)
/// ```
pub fn workspace_listing(workdir: &Path) -> String {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(workdir, workdir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut parts = String::new();
    for (rel_str, abs_path) in files {
        if !abs_path.is_file() {
            continue;
        }
        let name = abs_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "findings.json" {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&abs_path) {
            parts.push_str(&format!("\n### {rel_str}\n```\n{text}```\n"));
        }
    }
    parts
}

#[derive(Debug, Clone)]
struct OneshotWorkspaceContext {
    text: String,
    files: Vec<String>,
    truncated: bool,
}

fn oneshot_workspace_context(
    candidate: &Map<String, Value>,
    task_dir: &Path,
    workdir: &Path,
) -> Result<OneshotWorkspaceContext, Box<dyn std::error::Error>> {
    let mode = candidate
        .get("workspace_mode")
        .and_then(Value::as_str)
        .unwrap_or("full");
    match mode {
        "full" => Ok(full_workspace_context(workdir)),
        "review-context" => Ok(review_context_workspace(candidate, task_dir, workdir)),
        other => Err(format!("unsupported oneshot workspace_mode: {other}").into()),
    }
}

fn full_workspace_context(workdir: &Path) -> OneshotWorkspaceContext {
    OneshotWorkspaceContext {
        text: workspace_listing(workdir),
        files: Vec::new(),
        truncated: false,
    }
}

fn review_context_workspace(
    candidate: &Map<String, Value>,
    task_dir: &Path,
    workdir: &Path,
) -> OneshotWorkspaceContext {
    let total_limit = manifest_usize(candidate, "workspace_max_bytes", 120_000);
    let file_limit = manifest_usize(candidate, "workspace_file_bytes", 24_000);
    let mut text = String::new();
    let mut files = Vec::new();
    let mut truncated = false;

    if let Ok(intent) = std::fs::read_to_string(task_dir.join("intent.md")) {
        push_text_section(
            &mut text,
            "Task intent",
            &intent,
            file_limit,
            total_limit,
            &mut truncated,
        );
    }

    let Ok(diff) = std::fs::read_to_string(workdir.join("PR.diff")) else {
        return full_workspace_context(workdir);
    };
    if diff.trim().is_empty() {
        return full_workspace_context(workdir);
    }
    push_workspace_file(
        &mut text,
        &mut files,
        workdir,
        "PR.diff",
        file_limit,
        total_limit,
        &mut truncated,
    );

    for rel in changed_paths_from_diff(&diff) {
        push_workspace_file(
            &mut text,
            &mut files,
            workdir,
            &rel,
            file_limit,
            total_limit,
            &mut truncated,
        );
    }

    for rel in ["README.md", "pyproject.toml", "setup.py"] {
        push_workspace_file(
            &mut text,
            &mut files,
            workdir,
            rel,
            file_limit.min(8_000),
            total_limit,
            &mut truncated,
        );
    }

    if truncated {
        push_truncation_notice(&mut text, total_limit);
    }

    OneshotWorkspaceContext {
        text,
        files,
        truncated,
    }
}

fn manifest_usize(candidate: &Map<String, Value>, key: &str, default: usize) -> usize {
    candidate
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

fn changed_paths_from_diff(diff: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in diff.lines() {
        let Some(raw) = line.strip_prefix("+++ ") else {
            continue;
        };
        let Some(rel) = normalize_diff_new_path(raw) else {
            continue;
        };
        if is_safe_workspace_rel(&rel) && !paths.iter().any(|p| p == &rel) {
            paths.push(rel.to_string());
        }
    }
    paths
}

fn normalize_diff_new_path(raw: &str) -> Option<String> {
    let token = diff_path_token(raw);
    let token = strip_surrounding_quotes(token.trim());
    if token == "/dev/null" {
        return None;
    }
    let rel = token.strip_prefix("b/").unwrap_or(&token);
    if rel == "/dev/null" {
        None
    } else {
        Some(rel.to_string())
    }
}

fn diff_path_token(raw: &str) -> &str {
    let raw = raw.trim();
    if raw.starts_with('"') {
        let mut escaped = false;
        for (idx, ch) in raw.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                return &raw[..=idx];
            }
        }
    }
    raw.split('\t').next().unwrap_or(raw).trim()
}

fn strip_surrounding_quotes(raw: &str) -> String {
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        raw[1..raw.len() - 1].to_string()
    } else {
        raw.to_string()
    }
}

fn is_safe_workspace_rel(rel: &str) -> bool {
    let path = Path::new(rel);
    !rel.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn push_workspace_file(
    out: &mut String,
    files: &mut Vec<String>,
    workdir: &Path,
    rel: &str,
    file_limit: usize,
    total_limit: usize,
    truncated: &mut bool,
) {
    if files.iter().any(|p| p == rel) || !is_safe_workspace_rel(rel) {
        return;
    }
    let path = workdir.join(rel);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    if push_text_section(out, rel, &content, file_limit, total_limit, truncated) {
        files.push(rel.to_string());
    }
}

fn push_text_section(
    out: &mut String,
    title: &str,
    content: &str,
    file_limit: usize,
    total_limit: usize,
    truncated: &mut bool,
) -> bool {
    let header = format!("\n### {title}\n```\n");
    let footer = "```\n";
    let trunc_msg = "\n...<truncated>\n";
    let used = out.len();
    let fixed = header.len() + footer.len();
    if used.saturating_add(fixed) >= total_limit {
        *truncated = true;
        return false;
    }
    let available = total_limit.saturating_sub(used + fixed).min(file_limit);
    let (body, truncated_body) = if content.len() > available {
        if available <= trunc_msg.len() {
            *truncated = true;
            return false;
        }
        (utf8_prefix(content, available - trunc_msg.len()), true)
    } else {
        (content, false)
    };
    if body.is_empty() && truncated_body {
        *truncated = true;
        return false;
    }
    out.push_str(&header);
    out.push_str(body);
    if truncated_body {
        *truncated = true;
        out.push_str(trunc_msg);
    }
    out.push_str(footer);
    true
}

fn utf8_prefix(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }
    let mut end = 0;
    for (idx, ch) in content.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    &content[..end]
}

fn push_truncation_notice(out: &mut String, total_limit: usize) {
    let notice = "\n\nContext truncated by oneshot review-context limits.\n";
    if out.len().saturating_add(notice.len()) <= total_limit {
        out.push_str(notice);
    }
}

// ---------------------------------------------------------------------------
// extract_json_object
// ---------------------------------------------------------------------------

/// Pull the first parseable top-level JSON object out of model output.
///
/// Mirrors Python's brace-depth scan exactly:
/// ```python
/// start = text.find("{")
/// while start != -1:
///     depth = 0
///     for i in range(start, len(text)):
///         if text[i] == "{": depth += 1
///         elif text[i] == "}":
///             depth -= 1
///             if depth == 0:
///                 try: return json.loads(text[start : i + 1])
///                 except json.JSONDecodeError: break
///     start = text.find("{", start + 1)
/// raise ValueError("no JSON object found in model output")
/// ```
pub fn extract_json_object(text: &str) -> Result<Value, String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut start = 0usize;

    while let Some(rel) = text[start..].find('{') {
        let abs_start = start + rel;
        let mut depth: i32 = 0;
        let mut i = abs_start;
        while i < len {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let slice = &text[abs_start..=i];
                        if let Ok(v) = serde_json::from_str::<Value>(slice) {
                            return Ok(v);
                        }
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        start = abs_start + 1;
    }
    Err("no JSON object found in model output".to_string())
}

// ---------------------------------------------------------------------------
// candidate_env
// ---------------------------------------------------------------------------

/// Baseline process environment for candidate subprocesses.
///
/// Mirrors:
/// ```python
/// BASE_ENV_VARS = ("PATH", "HOME", "TERM", "LANG", "LC_ALL")
/// allow = candidate.get("env_allowlist", ["OPENROUTER_API_KEY"])
/// return {k: os.environ[k] for k in (*BASE_ENV_VARS, *allow) if k in os.environ}
/// ```
pub fn candidate_env(
    candidate: &Map<String, Value>,
    env: &HashMap<String, String>,
) -> HashMap<String, String> {
    let base: &[&str] = &["PATH", "HOME", "TERM", "LANG", "LC_ALL"];
    let allow: Vec<String> = match candidate.get("env_allowlist") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        _ => vec!["OPENROUTER_API_KEY".to_string()],
    };
    base.iter()
        .map(|k| k.to_string())
        .chain(allow)
        .filter_map(|k| env.get(&k).map(|v| (k, v.clone())))
        .collect()
}

// ---------------------------------------------------------------------------
// extract_pi_usage
// ---------------------------------------------------------------------------

/// Sum usage across assistant `message_end` events in `pi --mode json` output.
///
/// Mirrors Python exactly: sums `input`/`output`/`cacheRead` as `int(x or 0)`,
/// sums `cost.total` as `float`, returns `round(cost, 6)` via
/// `pycompat::round_half_even`. Provider is taken from the last seen assistant
/// message. Returns empty map if no qualifying events found.
pub fn extract_pi_usage(stdout_text: &str) -> Map<String, Value> {
    let mut tokens_in: i64 = 0;
    let mut tokens_out: i64 = 0;
    let mut cached: i64 = 0;
    let mut cost: f64 = 0.0;
    let mut provider: Option<String> = None;
    let mut found = false;

    for line in stdout_text.lines() {
        let line = line.trim();
        if !line.starts_with(r#"{"type":"message_end""#) {
            continue;
        }
        let event: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let msg = match event.get("message") {
            Some(m) => m,
            None => continue,
        };
        if msg.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        found = true;
        // provider: msg.get("provider") or previous provider
        if let Some(p) = msg.get("provider").and_then(Value::as_str) {
            provider = Some(p.to_string());
        }
        let usage = msg.get("usage").and_then(Value::as_object);
        if let Some(u) = usage {
            tokens_in += as_int_or_0(u.get("input"));
            tokens_out += as_int_or_0(u.get("output"));
            cached += as_int_or_0(u.get("cacheRead"));
            let total = u
                .get("cost")
                .and_then(|c| c.get("total"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            cost += total;
        }
    }

    if !found {
        return Map::new();
    }

    let mut out = Map::new();
    out.insert(
        "provider_served".to_string(),
        match provider {
            Some(p) => Value::String(p),
            None => Value::Null,
        },
    );
    out.insert(
        "tokens_prompt".to_string(),
        Value::Number(serde_json::Number::from(tokens_in)),
    );
    out.insert(
        "tokens_completion".to_string(),
        Value::Number(serde_json::Number::from(tokens_out)),
    );
    out.insert(
        "tokens_cached".to_string(),
        Value::Number(serde_json::Number::from(cached)),
    );
    out.insert(
        "cost_usd".to_string(),
        Value::Number(
            serde_json::Number::from_f64(round_half_even(cost, 6)).unwrap_or_else(|| 0.into()),
        ),
    );
    out
}

/// Python's `int(usage.get(key) or 0)` — coerce falsy to 0.
fn as_int_or_0(v: Option<&Value>) -> i64 {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => 0,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// build_pi_cmd
// ---------------------------------------------------------------------------

/// Compose the pi argv from the candidate's slots.
///
/// Mirrors Python's `build_pi_cmd(candidate)`:
/// - `--no-skills` unless `_skill_paths` is non-empty
/// - `--no-context-files` unless `_agents_md_text` is non-None
/// - `--append-system-prompt` or `--system-prompt` depending on
///   `system_prompt_mode`
pub fn build_pi_cmd(candidate: &Map<String, Value>) -> Vec<String> {
    let mut cmd = vec![
        "pi".to_string(),
        "-p".to_string(),
        "--mode".to_string(),
        "json".to_string(),
        "--no-session".to_string(),
        "--no-extensions".to_string(),
        "--no-prompt-templates".to_string(),
        "--no-themes".to_string(),
        "--provider".to_string(),
        candidate
            .get("provider_name")
            .and_then(Value::as_str)
            .unwrap_or("openrouter")
            .to_string(),
        "--model".to_string(),
        candidate
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ];

    // Skills: non-empty _skill_paths → add --skill flags; else --no-skills
    let has_skills = match candidate.get("_skill_paths") {
        Some(Value::Array(arr)) => !arr.is_empty(),
        _ => false,
    };
    if has_skills {
        if let Some(Value::Array(paths)) = candidate.get("_skill_paths") {
            for p in paths {
                if let Value::String(s) = p {
                    cmd.push("--skill".to_string());
                    cmd.push(s.clone());
                }
            }
        }
    } else {
        cmd.push("--no-skills".to_string());
    }

    // Agents MD: None → --no-context-files
    let has_agents_md = !matches!(candidate.get("_agents_md_text"), Some(Value::Null) | None);
    if !has_agents_md {
        cmd.push("--no-context-files".to_string());
    }

    // Thinking flag
    if let Some(v) = candidate.get("thinking").and_then(Value::as_str) {
        cmd.push("--thinking".to_string());
        cmd.push(v.to_string());
    }

    // Tools flag
    if let Some(Value::Array(tools)) = candidate.get("tools") {
        let tool_str: Vec<&str> = tools.iter().filter_map(Value::as_str).collect();
        if !tool_str.is_empty() {
            cmd.push("--tools".to_string());
            cmd.push(tool_str.join(","));
        }
    }

    // Prompt packet: system-prompt or append-system-prompt
    let packet_text = candidate
        .get("_packet_text")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !packet_text.is_empty() {
        let mode = candidate
            .get("system_prompt_mode")
            .and_then(Value::as_str)
            .unwrap_or("append");
        let flag = if mode == "replace" {
            "--system-prompt"
        } else {
            "--append-system-prompt"
        };
        cmd.push(flag.to_string());
        cmd.push(packet_text.to_string());
    }

    cmd
}

// ---------------------------------------------------------------------------
// prepare_workspace
// ---------------------------------------------------------------------------

/// Slot-driven workspace composition beyond the fixture copy.
///
/// Mirrors Python:
/// ```python
/// if candidate.get("_agents_md_text") is not None:
///     (workdir / "AGENTS.md").write_text(candidate["_agents_md_text"])
/// ```
pub fn prepare_workspace(
    candidate: &Map<String, Value>,
    workdir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    match candidate.get("_agents_md_text") {
        Some(Value::String(text)) => {
            std::fs::write(workdir.join("AGENTS.md"), text)?;
        }
        Some(Value::Null) | None => {}
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// summarize
// ---------------------------------------------------------------------------

/// Per-candidate, per-task reward distributions from a trials file.
///
/// Mirrors Python's `summarize(trials_path)` exactly, including:
/// - `round(sum/len, 4)` via `pycompat::round_half_even`
/// - field insertion order (composition_hash, kind, tasks, trials, errors,
///   cost_usd_total, cost_known)
/// - per-task `mean`, `min`, `max` and full `rewards`/`wall_ms` arrays
pub fn summarize(trials_path: &Path) -> Result<Map<String, Value>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(trials_path)?;
    let records: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

    // summary: candidate_id → candidate_summary
    let mut summary: Map<String, Value> = Map::new();

    for r in &records {
        let cand_id = r["candidate_id"].as_str().unwrap_or("").to_string();
        let task_id = r["task_id"].as_str().unwrap_or("").to_string();
        let reward = r["reward"].as_f64().unwrap_or(0.0);
        // Preserve the original JSON type of wall_ms (int or float) so that
        // serialisation matches Python's json.dumps (e.g. 9 stays 9, not 9.0).
        let wall_ms_val = r.get("wall_ms").cloned().unwrap_or(Value::Null);
        let _wall_ms = wall_ms_val.as_f64().unwrap_or(0.0);
        let cost_usd = r.get("cost_usd");
        let error = r.get("error");

        let c = summary.entry(cand_id).or_insert_with(|| {
            // Build the initial candidate struct in Python insertion order.
            let mut m = Map::new();
            m.insert(
                "composition_hash".to_string(),
                r.get("composition_hash").cloned().unwrap_or(Value::Null),
            );
            m.insert(
                "kind".to_string(),
                r.get("candidate_kind").cloned().unwrap_or(Value::Null),
            );
            m.insert("tasks".to_string(), Value::Object(Map::new()));
            m.insert(
                "trials".to_string(),
                Value::Number(serde_json::Number::from(0i64)),
            );
            m.insert(
                "errors".to_string(),
                Value::Number(serde_json::Number::from(0i64)),
            );
            m.insert(
                "cost_usd_total".to_string(),
                Value::Number(serde_json::Number::from_f64(0.0).unwrap()),
            );
            m.insert("cost_known".to_string(), Value::Bool(true));
            Value::Object(m)
        });

        let c = c.as_object_mut().unwrap();

        // trials += 1
        let trials = c["trials"].as_i64().unwrap_or(0) + 1;
        c.insert(
            "trials".to_string(),
            Value::Number(serde_json::Number::from(trials)),
        );

        // errors += 1 if error is truthy
        let has_error = match error {
            Some(Value::Null) | None => false,
            Some(Value::Bool(false)) => false,
            Some(Value::String(s)) if s.is_empty() => false,
            _ => true,
        };
        if has_error {
            let errors = c["errors"].as_i64().unwrap_or(0) + 1;
            c.insert(
                "errors".to_string(),
                Value::Number(serde_json::Number::from(errors)),
            );
        }

        // tasks[task_id].rewards.append(reward), .wall_ms.append(wall_ms)
        let tasks = c["tasks"].as_object_mut().unwrap();
        let t = tasks
            .entry(task_id)
            .or_insert_with(|| {
                let mut m = Map::new();
                m.insert("rewards".to_string(), Value::Array(vec![]));
                m.insert("wall_ms".to_string(), Value::Array(vec![]));
                Value::Object(m)
            })
            .as_object_mut()
            .unwrap();
        t["rewards"]
            .as_array_mut()
            .unwrap()
            .push(Value::Number(serde_json::Number::from_f64(reward).unwrap()));
        // Push the original value (preserving int vs float type from JSONL).
        t["wall_ms"].as_array_mut().unwrap().push(wall_ms_val);

        // cost tracking
        match cost_usd {
            Some(Value::Null) | None => {
                c.insert("cost_known".to_string(), Value::Bool(false));
            }
            Some(v) => {
                let cost_val = v.as_f64().unwrap_or(0.0);
                let current = c["cost_usd_total"].as_f64().unwrap_or(0.0);
                c.insert(
                    "cost_usd_total".to_string(),
                    Value::Number(
                        serde_json::Number::from_f64(current + cost_val)
                            .unwrap_or_else(|| 0.into()),
                    ),
                );
            }
        }
    }

    // Post-processing: compute per-candidate and per-task aggregates.
    for c in summary.values_mut() {
        let c = c.as_object_mut().unwrap();

        // All rewards across all tasks
        let all_rewards: Vec<f64> = c["tasks"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|t| {
                t["rewards"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_f64)
            })
            .collect();

        let n = all_rewards.len() as f64;
        let sum_r: f64 = all_rewards.iter().sum();
        let reward_mean = round_half_even(sum_r / n, 4);
        c.insert(
            "reward_mean".to_string(),
            Value::Number(serde_json::Number::from_f64(reward_mean).unwrap()),
        );

        let total_cost = c["cost_usd_total"].as_f64().unwrap_or(0.0);
        c.insert(
            "cost_usd_total".to_string(),
            Value::Number(
                serde_json::Number::from_f64(round_half_even(total_cost, 6))
                    .unwrap_or_else(|| 0.into()),
            ),
        );

        // Per-task: mean, min, max
        let tasks = c["tasks"].as_object_mut().unwrap();
        for t in tasks.values_mut() {
            let t = t.as_object_mut().unwrap();
            let rewards: Vec<f64> = t["rewards"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_f64)
                .collect();
            let nt = rewards.len() as f64;
            let st: f64 = rewards.iter().sum();
            let mean_r = round_half_even(st / nt, 4);
            let min_r: f64 = rewards.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_r: f64 = rewards.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

            t.insert(
                "mean".to_string(),
                Value::Number(serde_json::Number::from_f64(mean_r).unwrap()),
            );
            t.insert(
                "min".to_string(),
                Value::Number(serde_json::Number::from_f64(min_r).unwrap()),
            );
            t.insert(
                "max".to_string(),
                Value::Number(serde_json::Number::from_f64(max_r).unwrap()),
            );
        }
    }

    Ok(summary)
}

// ---------------------------------------------------------------------------
// run_null / run_oracle (deterministic halves of the executor functions)
// ---------------------------------------------------------------------------

/// Write an empty findings file. Mirrors `run_null` without the `workdir`
/// I/O being the test surface — returns the JSON string the function would
/// write, so the caller can assert it or write it.
pub fn null_findings_json() -> &'static str {
    "{\"findings\": []}\n"
}

/// Copy oracle findings from `task_dir/solution/findings.json` to
/// `workdir/findings.json`. Returns the findings JSON string.
pub fn oracle_findings(task_dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let src = task_dir.join("solution").join("findings.json");
    Ok(std::fs::read_to_string(src)?)
}

// ---------------------------------------------------------------------------
// LIVE EXECUTION GLUE — added to run.rs after the deterministic core.
// These functions own I/O, subprocesses, temp dirs, and wall-clock time.
// They are NOT parity-tested individually (wall-clock, run IDs, etc. are
// inherently non-deterministic); instead the e2e test in
// tests/parity_run_e2e.rs runs the FULL pipeline against Python and compares
// the deterministic fields of the output records.
// ---------------------------------------------------------------------------

/// Current UTC timestamp formatted as `%Y%m%dT%H%M%SZ`.
///
/// Wall-clock, not parity-tested. Mirrors Python's:
/// ```python
/// datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
/// ```
pub fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = crate::pycompat::civil_from_days(days);
    format!("{y:04}{mo:02}{d:02}T{h:02}{m:02}{s:02}Z")
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")` →
/// `"2026-06-16T12:00:00+00:00"`. The compact run-id form is [`utc_stamp`];
/// this `+00:00` ISO form is what the `ts_start`/`ts_end` trial-record fields use.
fn iso_stamp() -> String {
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

/// Recursive `shutil.copytree(src, dst, dirs_exist_ok=True)` equivalent.
///
/// Mirrors Python's copytree: copies all files and dirs from `src` into `dst`,
/// creating `dst` if needed (dirs_exist_ok — merges rather than refusing).
pub fn copy_dir(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Run a `pi` candidate against a task workspace.
///
/// Mirrors Python's `run_pi`: composes the argv via `build_pi_cmd`, appends
/// the instruction message, runs with `env_clear()` + `candidate_env`, and
/// captures stdout/stderr.
///
/// On non-zero exit: returns `Err("pi exited {code}: {stderr tail 400}")`.
/// Merges `extract_pi_usage(stdout)` into `record` on success.
///
/// # Per-trial timeout
/// Python passed `timeout=candidate.get("timeout_sec", 600)` to
/// `subprocess.run`. `std::process::Command` has no built-in timeout, so
/// [`run_with_timeout`] reproduces it dependency-free: the child is killed if
/// it outlives `candidate["timeout_sec"]` (default 600s) and the trial is
/// recorded as `Err("pi timed out after {n}s (killed)")`. Without this a hung
/// agent blocks the whole search indefinitely.
pub fn run_pi(
    candidate: &Map<String, Value>,
    instruction: &str,
    _task_dir: &Path,
    workdir: &Path,
    record: &mut Map<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    prepare_workspace(candidate, workdir)?;

    let mut argv = build_pi_cmd(candidate);
    // Append the message: instruction + workspace note (matches Python exactly).
    argv.push(format!(
        "{instruction}\n\nThe workspace is the current working directory."
    ));

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cenv = candidate_env(candidate, &env);

    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(workdir)
        .env_clear()
        .envs(&cenv)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // `timeout_sec` comes from candidate JSON; treat 0 (or absent) as the
    // 600s default so a misconfigured `0` can't kill the child before it runs.
    let timeout_sec = candidate
        .get("timeout_sec")
        .and_then(Value::as_u64)
        .filter(|&s| s > 0)
        .unwrap_or(600);

    let (status_opt, stdout, stderr) =
        run_with_timeout(cmd, std::time::Duration::from_secs(timeout_sec))?;

    let stdout_text = String::from_utf8_lossy(&stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(&stderr).into_owned();

    // Record exit code, transcript, and usage on every path so a timed-out or
    // failed trial still surfaces whatever the agent emitted. Insertion order
    // matches the original to keep run records byte-stable.
    let exit_code = status_opt.as_ref().and_then(|s| s.code()).unwrap_or(-1);
    record.insert(
        "agent_exit_code".to_string(),
        Value::Number(serde_json::Number::from(exit_code)),
    );
    record.insert(
        "_transcript_text".to_string(),
        Value::String(stdout_text.clone()),
    );
    for (k, v) in extract_pi_usage(&stdout_text) {
        record.insert(k, v);
    }

    match status_opt {
        None => Err(format!("pi timed out after {timeout_sec}s (killed)").into()),
        Some(status) if !status.success() => {
            let code = status.code().unwrap_or(-1);
            let tail: String = stderr_text
                .chars()
                .rev()
                .take(400)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            Err(format!("pi exited {code}: {tail}").into())
        }
        Some(_) => Ok(()),
    }
}

/// Run a prepared `Command` to completion, or kill it after `timeout`.
///
/// `std::process::Command` has no built-in timeout; this is the dependency-free
/// equivalent of Python's `subprocess.run(timeout=...)`. stdout/stderr are
/// drained on background threads so a child that fills a pipe buffer cannot
/// deadlock the wait, and the bytes are collected through channels with a short
/// grace period (applied to BOTH the clean-exit and kill paths) so a surviving
/// grandchild that holds a pipe open can never hang the caller. Returns
/// `Ok((None, ..))` if the child was killed for exceeding `timeout`,
/// `Ok((Some(status), ..))` otherwise.
///
/// Limitation: a killed child's descendants are not reaped (no process-group
/// kill), so a long-lived orphan that keeps a pipe open leaves its reader thread
/// blocked on `read_to_end` until it exits — a bounded, usually-transient leak
/// of one thread + two FDs per occurrence, never a hang. Process-group reaping
/// is a follow-up.
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<(Option<std::process::ExitStatus>, Vec<u8>, Vec<u8>)> {
    use std::io::Read;
    use std::sync::mpsc;

    let mut child = cmd.spawn()?;
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");

    let (otx, orx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = otx.send(buf);
    });
    let (etx, erx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = etx.send(buf);
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(s) => break Some(s),
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };

    // Collect the drained bytes, bounding BOTH the clean-exit and kill paths.
    // A child that has exited (cleanly or via our kill) closed its own write
    // ends, so the readers reach EOF within microseconds — at most one ~64KB
    // pipe buffer remains — and the grace is never hit on a healthy run (the
    // large-output test confirms a 200KB drain returns instantly). The bound
    // exists solely to defeat an abnormal descendant that inherited a pipe and
    // outlives its parent: without it, `read_to_end` would wait on an EOF that
    // never comes and hang the whole search forever. A bounded, possibly-partial
    // trial is strictly better than an unbounded hang.
    // Share ONE deadline across both pipes so total drain time is bounded by
    // `grace`, not 2×grace: a killed child whose grandchild holds both pipes
    // would otherwise block `grace` on stdout and then `grace` again on stderr.
    let grace = std::time::Duration::from_secs(3);
    let deadline = std::time::Instant::now() + grace;
    let stdout = orx
        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        .unwrap_or_default();
    let stderr = erx
        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        .unwrap_or_default();
    Ok((status, stdout, stderr))
}

/// Coarse, tokenizer-free pre-flight for the one-shot probe: does a prompt of
/// `prompt_chars` characters plus `max_tokens` of completion fit inside
/// `context_window` tokens (≈4 chars/token)? Real-repo arenas dump the entire
/// workspace into the prompt — hundreds of thousands of tokens — which the
/// model rejects with an opaque HTTP 400. Returning `Err` lets [`run_oneshot`]
/// skip the doomed request with a legible reason and zero spend.
#[cfg(feature = "http")]
fn oneshot_context_fits(
    prompt_chars: usize,
    max_tokens: i64,
    context_window: u64,
) -> Result<(), String> {
    const CHARS_PER_TOKEN: usize = 4;
    // Round up: a prompt one char past a token boundary still costs that whole
    // token, so flooring would admit a request one token over the estimate.
    let est_prompt_tokens = prompt_chars.div_ceil(CHARS_PER_TOKEN) as u64;
    let need = est_prompt_tokens.saturating_add(max_tokens.max(0) as u64);
    if need > context_window {
        return Err(format!(
            "estimated {est_prompt_tokens} prompt + {max_tokens} completion tokens \
             exceed model context {context_window}; the one-shot probe cannot ingest \
             this workspace (a tool-using agent is required for real-repo arenas)"
        ));
    }
    Ok(())
}

/// Port of `runner/run.py::run_oneshot`.
///
/// Calls OpenRouter's chat-completions endpoint with the candidate's model,
/// extracts findings, writes `workdir/findings.json`, and merges provider/
/// token/cost fields into `record`.  Requires `OPENROUTER_API_KEY` in the
/// environment.  This is a live-I/O boundary; it is NOT parity-tested.
#[cfg(feature = "http")]
pub fn run_oneshot(
    candidate: &Map<String, Value>,
    instruction: &str,
    task_dir: &Path,
    workdir: &Path,
    record: &mut Map<String, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

    let key = std::env::var("OPENROUTER_API_KEY").map_err(|_| "OPENROUTER_API_KEY is not set")?;

    let workspace_context = oneshot_workspace_context(candidate, task_dir, workdir)?;
    let workspace_mode = candidate
        .get("workspace_mode")
        .and_then(Value::as_str)
        .unwrap_or("full");
    let prompt = format!(
        "{instruction}\n\n## Workspace files\n{}\nRespond with ONLY the findings JSON object, no prose.",
        workspace_context.text
    );

    let system = candidate
        .get("_packet_text")
        .and_then(Value::as_str)
        .unwrap_or("You are a precise code-review agent. Output only valid JSON.");

    let temperature = candidate
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or(0.2);
    // A non-positive max_tokens is meaningless and would be sent verbatim into
    // the request body; treat 0/negative/absent as the 8192 default so the
    // pre-flight estimate and the actual request agree.
    let max_tokens = candidate
        .get("max_tokens")
        .and_then(Value::as_i64)
        .filter(|&t| t > 0)
        .unwrap_or(8192);

    // Pre-flight: a real-repo workspace can exceed the model context window,
    // which OpenRouter rejects as a bare HTTP 400. Skip the doomed call instead
    // of burning one failed round-trip per task.
    const DEFAULT_CONTEXT_WINDOW: u64 = 256_000;
    let context_window = candidate
        .get("context_window")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);
    record.insert(
        "oneshot_workspace_mode".to_string(),
        Value::String(workspace_mode.to_string()),
    );
    record.insert(
        "oneshot_prompt_chars".to_string(),
        Value::Number(serde_json::Number::from(prompt.chars().count() as u64)),
    );
    record.insert(
        "oneshot_context_window".to_string(),
        Value::Number(serde_json::Number::from(context_window)),
    );
    record.insert(
        "oneshot_workspace_truncated".to_string(),
        Value::Bool(workspace_context.truncated),
    );
    record.insert(
        "oneshot_included_files".to_string(),
        Value::Array(
            workspace_context
                .files
                .iter()
                .map(|path| Value::String(path.clone()))
                .collect(),
        ),
    );
    if let Err(reason) = oneshot_context_fits(
        system.chars().count() + prompt.chars().count(),
        max_tokens,
        context_window,
    ) {
        record.insert("cost_usd".to_string(), Value::from(0.0));
        return Err(format!("one-shot skipped: {reason}").into());
    }

    let mut body = serde_json::json!({
        "model": candidate.get("model").cloned().unwrap_or(Value::Null),
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": prompt},
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "usage": {"include": true},
    });
    if let Some(provider) = candidate.get("provider") {
        body.as_object_mut()
            .unwrap()
            .insert("provider".to_string(), provider.clone());
    }

    let timeout_sec = candidate
        .get("timeout_sec")
        .and_then(Value::as_u64)
        .unwrap_or(300);

    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(timeout_sec))
        .build();

    let resp = agent
        .post(OPENROUTER_URL)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_json(&body);

    let payload: Value = match resp {
        Ok(r) => r.into_json()?,
        Err(ureq::Error::Status(code, r)) => {
            // A rejected request bills nothing — record $0 so known-spend stays honest.
            record.insert("cost_usd".to_string(), Value::from(0.0));
            let reason = r.status_text().to_string();
            return Err(format!("HTTP Error {code}: {reason}").into());
        }
        Err(e) => return Err(e.into()),
    };

    let usage = payload
        .get("usage")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    record.insert(
        "provider_served".to_string(),
        payload.get("provider").cloned().unwrap_or(Value::Null),
    );
    record.insert(
        "tokens_prompt".to_string(),
        usage.get("prompt_tokens").cloned().unwrap_or(Value::Null),
    );
    record.insert(
        "tokens_completion".to_string(),
        usage
            .get("completion_tokens")
            .cloned()
            .unwrap_or(Value::Null),
    );
    record.insert(
        "tokens_cached".to_string(),
        usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    record.insert(
        "cost_usd".to_string(),
        usage.get("cost").cloned().unwrap_or(Value::Null),
    );

    let choice = payload
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or("oneshot response missing choices")?;
    let content = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            let fr = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("?");
            format!("model returned empty content (finish_reason={fr})")
        })?;

    if content.is_empty() {
        let fr = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("?");
        return Err(format!("model returned empty content (finish_reason={fr})").into());
    }

    record.insert(
        "_response_text".to_string(),
        Value::String(content.to_string()),
    );

    let findings = extract_json_object(content)?;
    std::fs::write(
        workdir.join("findings.json"),
        serde_json::to_string_pretty(&findings)?,
    )?;

    Ok(())
}

/// Inputs resolved before the trial loop, passed to `run_arena`.
pub struct ArenaInputs {
    pub candidate_path: PathBuf,
    pub arena_dir: PathBuf,
    pub task_filter: Option<std::collections::HashSet<String>>,
    pub trials: u32,
    pub exp_dir: Option<PathBuf>,
    pub split: String,
    pub is_final: bool,
    pub max_errors: Option<usize>,
    pub repo_root: PathBuf,
    pub runs_root: PathBuf,
}

/// Faithful port of `runner/run.py`'s `main()` body as a reusable library
/// function.  Returns the experiment directory path on success.
///
/// Reproduces the trial loop exactly:
/// - per-trial record field set and insertion order (mirroring Python's dict)
/// - tempdir workspace + `environment/` copy
/// - executor dispatch by `candidate["kind"]` (null/oracle/pi/oneshot)
/// - grader-tamper check via `tree_digest` before+after
/// - score via `crate::score::score` (ScoreResult fields merged into record)
/// - failed-trial zeroing (error → reward=0, recall=0, matched=[], etc.)
/// - findings extraction from workdir/findings.json
/// - artifacts dir + `artifacts.index` append
/// - `trials.jsonl` append
/// - `summarize` → `summary.json`
///
/// Non-deterministic fields (`run_id`, `ts_start`, `ts_end`, `wall_ms`,
/// `artifacts`, `harness_version`) are always excluded from parity assertions.
pub fn run_arena(inputs: ArenaInputs) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = load_candidate(&inputs.candidate_path, &inputs.repo_root)?;
    let arena_dir = &inputs.arena_dir;
    let arena = load_toml(&arena_dir.join("arena.toml"))?;

    validate_arena_for_local_execution(&arena)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    let kind = candidate
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let task_dirs = select_tasks(
        arena_dir,
        &arena,
        &inputs.split,
        inputs.task_filter.as_ref(),
        inputs.is_final,
    )
    .map_err(|e| e.as_str().to_string())?;

    if task_dirs.is_empty() {
        return Err("no tasks matched".into());
    }

    // Pre-validate all tasks (mirrors Python's up-front loop before the trial loop).
    let mut task_inputs: Vec<(PathBuf, String)> = Vec::new();
    for task_dir in &task_dirs {
        validate_task_dir(task_dir)?;
        let instruction = task_instruction(arena_dir, &arena, task_dir)?;
        let env_dir = task_dir.join("environment");
        validate_no_hidden_absolute_paths(&candidate, task_dir, &instruction, &env_dir)?;
        task_inputs.push((task_dir.clone(), instruction));
    }

    let stamp = utc_stamp();
    let candidate_id = candidate
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let exp_dir = match &inputs.exp_dir {
        Some(d) => d.clone(),
        None => inputs.runs_root.join(format!("{stamp}-{candidate_id}")),
    };

    std::fs::create_dir_all(exp_dir.join("compositions"))?;

    // Write composition snapshot (mirrors Python).
    let mut snapshot: Map<String, Value> = Map::new();
    for (k, v) in &candidate {
        if !k.starts_with('_') {
            snapshot.insert(k.clone(), v.clone());
        }
    }
    snapshot.insert("composition_hash".to_string(), candidate["_hash"].clone());
    snapshot.insert(
        "prompt_packet_text".to_string(),
        candidate["_packet_text"].clone(),
    );
    snapshot.insert(
        "skills_texts".to_string(),
        candidate["_skills_texts"].clone(),
    );
    snapshot.insert(
        "agents_md_text".to_string(),
        candidate["_agents_md_text"].clone(),
    );
    // harness_version: not ported (would need subprocess call to `pi --version`).
    snapshot.insert("harness_version".to_string(), Value::Null);
    snapshot.insert(
        "runner_version".to_string(),
        Value::String("0.1.0".to_string()),
    );
    std::fs::write(
        exp_dir
            .join("compositions")
            .join(format!("{candidate_id}.json")),
        serde_json::to_string_pretty(&Value::Object(snapshot))?,
    )?;

    let trials_path = exp_dir.join("trials.jsonl");
    let mut all_records: Vec<Map<String, Value>> = Vec::new();
    let mut total_errors: usize = 0;

    'outer: for (task_dir, instruction) in &task_inputs {
        let task_id = task_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let grader_digest = tree_digest(&[&task_dir.join("tests"), &task_dir.join("solution")]);

        for trial in 1..=inputs.trials {
            // Build record in Python's insertion order.
            let mut record: Map<String, Value> = Map::new();
            record.insert(
                "run_id".to_string(),
                Value::String(format!("{stamp}-{candidate_id}-{task_id}-t{trial}")),
            );
            record.insert("ts_start".to_string(), Value::String(iso_stamp()));
            record.insert(
                "runner_version".to_string(),
                Value::String("0.1.0".to_string()),
            );
            record.insert(
                "arena_id".to_string(),
                arena.get("id").cloned().unwrap_or(Value::Null),
            );
            record.insert(
                "arena_version".to_string(),
                arena.get("version").cloned().unwrap_or(Value::Null),
            );
            record.insert(
                "taskspec".to_string(),
                arena.get("taskspec").cloned().unwrap_or(Value::Null),
            );
            record.insert("task_id".to_string(), Value::String(task_id.clone()));
            record.insert(
                "trial".to_string(),
                Value::Number(serde_json::Number::from(trial)),
            );
            record.insert(
                "candidate_id".to_string(),
                Value::String(candidate_id.clone()),
            );
            record.insert("candidate_kind".to_string(), Value::String(kind.clone()));
            record.insert("composition_hash".to_string(), candidate["_hash"].clone());
            record.insert("harness_version".to_string(), Value::Null);
            record.insert(
                "model".to_string(),
                candidate.get("model").cloned().unwrap_or(Value::Null),
            );
            record.insert("provider_served".to_string(), Value::Null);
            record.insert("tokens_prompt".to_string(), Value::Null);
            record.insert("tokens_completion".to_string(), Value::Null);
            record.insert("tokens_cached".to_string(), Value::Null);
            record.insert("cost_usd".to_string(), Value::Null);
            record.insert("error".to_string(), Value::Null);

            // Create temp workspace and copy environment/.
            // Use stamp + candidate_id + task_id + trial to avoid collisions
            // when multiple run_arena calls execute concurrently in tests.
            let workdir = std::env::temp_dir().join(format!(
                "threshold-{}-{candidate_id}-{task_id}-t{trial}",
                stamp
            ));
            std::fs::create_dir_all(&workdir)?;
            let t0 = std::time::Instant::now();

            let exec_result = (|| -> Result<(), Box<dyn std::error::Error>> {
                copy_dir(&task_dir.join("environment"), &workdir)?;
                match kind.as_str() {
                    "null" => {
                        std::fs::write(workdir.join("findings.json"), null_findings_json())?;
                    }
                    "oracle" => {
                        let findings = oracle_findings(task_dir)?;
                        std::fs::write(workdir.join("findings.json"), findings)?;
                    }
                    // The incumbent (055) executes as a pi agent; its distinct
                    // kind only tags it as a reference (excluded from the Pareto
                    // front, recommendation, and mutation) and the cert baseline.
                    "pi" | "incumbent" => {
                        run_pi(&candidate, instruction, task_dir, &workdir, &mut record)?;
                    }
                    "oneshot" => {
                        #[cfg(feature = "http")]
                        {
                            run_oneshot(&candidate, instruction, task_dir, &workdir, &mut record)?;
                        }
                        #[cfg(not(feature = "http"))]
                        {
                            return Err("oneshot kind requires the 'http' feature".into());
                        }
                    }
                    other => {
                        return Err(format!("unknown candidate kind: {other:?}").into());
                    }
                }
                Ok(())
            })();

            if let Err(e) = exec_result {
                record.insert("error".to_string(), Value::String(e.to_string()));
            }

            let wall_ms = t0.elapsed().as_millis() as i64;
            record.insert(
                "wall_ms".to_string(),
                Value::Number(serde_json::Number::from(wall_ms)),
            );

            // ts_end
            record.insert("ts_end".to_string(), Value::String(iso_stamp()));

            // Grader tamper check.
            if tree_digest(&[&task_dir.join("tests"), &task_dir.join("solution")]) != grader_digest
            {
                record.insert(
                    "error".to_string(),
                    Value::String("grader files modified during run; trial voided".to_string()),
                );
            }

            let has_error = matches!(record.get("error"), Some(v) if !v.is_null() && v != &Value::String(String::new()));

            if has_error {
                // Failed trials never earn reward.
                record.insert(
                    "reward".to_string(),
                    Value::Number(serde_json::Number::from_f64(0.0).unwrap()),
                );
                record.insert(
                    "recall".to_string(),
                    Value::Number(serde_json::Number::from_f64(0.0).unwrap()),
                );
                record.insert("matched".to_string(), Value::Array(vec![]));
                record.insert(
                    "false_positives".to_string(),
                    Value::Number(serde_json::Number::from(0i64)),
                );
                record.insert("expected_defects".to_string(), Value::Null);
                record.insert("scorer_error".to_string(), Value::Null);
            } else {
                match crate::score::score(
                    &workdir.join("findings.json"),
                    &task_dir.join("tests").join("expected.json"),
                ) {
                    Ok(verdict) => {
                        record.insert(
                            "reward".to_string(),
                            Value::Number(serde_json::Number::from_f64(verdict.reward).unwrap()),
                        );
                        record.insert(
                            "recall".to_string(),
                            Value::Number(serde_json::Number::from_f64(verdict.recall).unwrap()),
                        );
                        record.insert(
                            "matched".to_string(),
                            Value::Array(
                                verdict
                                    .matched
                                    .iter()
                                    .map(|s| Value::String(s.clone()))
                                    .collect(),
                            ),
                        );
                        record.insert(
                            "false_positives".to_string(),
                            Value::Number(serde_json::Number::from(verdict.false_positives)),
                        );
                        record.insert(
                            "expected_defects".to_string(),
                            Value::Number(serde_json::Number::from(
                                verdict.expected_defects as u64,
                            )),
                        );
                        record.insert(
                            "scorer_error".to_string(),
                            match verdict.error {
                                Some(e) => Value::String(e),
                                None => Value::Null,
                            },
                        );
                    }
                    Err(e) => {
                        // Answer key load failure: record error, zero reward.
                        record.insert("error".to_string(), Value::String(format!("scorer: {e}")));
                        record.insert(
                            "reward".to_string(),
                            Value::Number(serde_json::Number::from_f64(0.0).unwrap()),
                        );
                        record.insert(
                            "recall".to_string(),
                            Value::Number(serde_json::Number::from_f64(0.0).unwrap()),
                        );
                        record.insert("matched".to_string(), Value::Array(vec![]));
                        record.insert(
                            "false_positives".to_string(),
                            Value::Number(serde_json::Number::from(0i64)),
                        );
                        record.insert("expected_defects".to_string(), Value::Null);
                        record.insert("scorer_error".to_string(), Value::Null);
                    }
                }
            }

            // findings from workdir/findings.json
            let findings_file = workdir.join("findings.json");
            if findings_file.exists() {
                match std::fs::read_to_string(&findings_file)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                {
                    Some(v) => {
                        record.insert(
                            "findings".to_string(),
                            v.get("findings").cloned().unwrap_or(Value::Null),
                        );
                    }
                    None => {
                        record.insert("findings".to_string(), Value::Null);
                    }
                }
            }

            // Artifacts dir.
            let art_dir = exp_dir
                .join("artifacts")
                .join(&candidate_id)
                .join(format!("{task_id}-t{trial}-{stamp}"));
            std::fs::create_dir_all(&art_dir)?;

            // Pop _transcript_text and _response_text before recording.
            let transcript = record.remove("_transcript_text");
            let response = record.remove("_response_text");
            if let Some(Value::String(t)) = transcript {
                if !t.is_empty() {
                    std::fs::write(art_dir.join("transcript.txt"), t)?;
                }
            }
            if let Some(Value::String(r)) = response {
                if !r.is_empty() {
                    std::fs::write(art_dir.join("response.txt"), r)?;
                }
            }
            if findings_file.exists() {
                let _ = std::fs::copy(&findings_file, art_dir.join("findings.json"));
            }

            // artifacts field: relative path from exp_dir.
            let art_rel = art_dir
                .strip_prefix(&exp_dir)
                .unwrap_or(&art_dir)
                .to_string_lossy()
                .into_owned();
            record.insert("artifacts".to_string(), Value::String(art_rel.clone()));

            // artifacts.index append.
            let run_id = record
                .get("run_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut idx = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(exp_dir.join("artifacts.index"))?;
            use std::io::Write as IoWrite;
            writeln!(idx, "{run_id}\t{art_rel}")?;

            // Clean up workspace.
            let _ = std::fs::remove_dir_all(&workdir);

            // Append to trials.jsonl.
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&trials_path)?;
            writeln!(
                f,
                "{}",
                serde_json::to_string(&Value::Object(record.clone()))?
            )?;

            all_records.push(record.clone());

            // Error tracking for max_errors early stop.
            let this_error = matches!(record.get("error"), Some(v) if !v.is_null() && v != &Value::String(String::new()));
            if this_error {
                total_errors += 1;
            }

            if let Some(max_e) = inputs.max_errors {
                if total_errors >= max_e {
                    break 'outer;
                }
            }
        }
    }

    // summarize → summary.json
    let summary = summarize(&trials_path)?;
    std::fs::write(
        exp_dir.join("summary.json"),
        serde_json::to_string_pretty(&Value::Object(summary))?,
    )?;

    Ok(exp_dir)
}

// ---------------------------------------------------------------------------
// Unit tests (porting tests/test_run.py inline assertions)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const PI_TRANSCRIPT: &str = r#"{"type":"session","version":3,"id":"x","timestamp":"t","cwd":"/tmp"}
{"type":"message_end","message":{"role":"user","content":[]}}
{"type":"message_end","message":{"role":"assistant","provider":"openrouter","usage":{"input":397,"output":26,"cacheRead":10,"cacheWrite":0,"totalTokens":423,"cost":{"input":0.0002,"output":0.00008,"total":0.00028}}}}
{"type":"message_end","message":{"role":"assistant","provider":"openrouter","usage":{"input":500,"output":40,"cacheRead":0,"cacheWrite":0,"totalTokens":540,"cost":{"total":0.0005}}}}
"#;

    // -----------------------------------------------------------------------
    // extract_json_object
    // -----------------------------------------------------------------------

    #[test]
    fn extract_json_object_plain() {
        let v = extract_json_object(r#"{"findings": []}"#).unwrap();
        assert_eq!(v, serde_json::json!({"findings": []}));
    }

    #[test]
    fn extract_json_object_with_prose_and_fences() {
        let text = "Sure! Here you go:\n```json\n{\"findings\": [{\"a\": 1}]}\n```\nDone.";
        let v = extract_json_object(text).unwrap();
        assert_eq!(v, serde_json::json!({"findings": [{"a": 1}]}));
    }

    #[test]
    fn extract_json_object_skips_broken_prefix() {
        let text = r#"{broken {"findings": []}"#;
        let v = extract_json_object(text).unwrap();
        assert_eq!(v, serde_json::json!({"findings": []}));
    }

    #[test]
    fn extract_json_object_raises_when_absent() {
        assert!(extract_json_object("no json here").is_err());
    }

    // -----------------------------------------------------------------------
    // extract_pi_usage
    // -----------------------------------------------------------------------

    #[test]
    fn extract_pi_usage_sums_assistant_message_ends() {
        let usage = extract_pi_usage(PI_TRANSCRIPT);
        assert_eq!(usage["tokens_prompt"].as_i64().unwrap(), 897);
        assert_eq!(usage["tokens_completion"].as_i64().unwrap(), 66);
        assert_eq!(usage["tokens_cached"].as_i64().unwrap(), 10);
        assert!(
            (usage["cost_usd"].as_f64().unwrap() - 0.00078).abs() < 1e-9,
            "cost_usd = {}",
            usage["cost_usd"]
        );
        assert_eq!(usage["provider_served"].as_str().unwrap(), "openrouter");
    }

    #[test]
    fn extract_pi_usage_empty_on_no_events() {
        let usage = extract_pi_usage("plain text\n{\"type\":\"other\"}");
        assert!(usage.is_empty());
    }

    // -----------------------------------------------------------------------
    // candidate_env
    // -----------------------------------------------------------------------

    #[test]
    fn candidate_env_withholds_unrelated_secrets() {
        let mut env = HashMap::new();
        env.insert("GITHUB_TOKEN".to_string(), "sekret".to_string());
        env.insert("OPENAI_API_KEY".to_string(), "sekret2".to_string());
        env.insert("OPENROUTER_API_KEY".to_string(), "or-key".to_string());
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        let candidate = Map::new();
        let result = candidate_env(&candidate, &env);
        assert!(!result.contains_key("GITHUB_TOKEN"));
        assert!(!result.contains_key("OPENAI_API_KEY"));
        assert_eq!(result["OPENROUTER_API_KEY"], "or-key");
        assert!(result.contains_key("PATH"));
    }

    #[test]
    fn candidate_env_respects_manifest_allowlist() {
        let mut env = HashMap::new();
        env.insert("CUSTOM_KEY".to_string(), "v".to_string());
        env.insert("OPENROUTER_API_KEY".to_string(), "or-key".to_string());
        let mut candidate = Map::new();
        candidate.insert(
            "env_allowlist".to_string(),
            Value::Array(vec![Value::String("CUSTOM_KEY".to_string())]),
        );
        let result = candidate_env(&candidate, &env);
        assert_eq!(result["CUSTOM_KEY"], "v");
        assert!(!result.contains_key("OPENROUTER_API_KEY"));
    }

    // -----------------------------------------------------------------------
    // tree_digest
    // -----------------------------------------------------------------------

    #[test]
    fn source_repo_reads_the_task_toml_label() {
        let tmp = std::env::temp_dir().join(format!("threshold-srcrepo-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // Labeled task → Some(repo).
        std::fs::write(
            tmp.join("task.toml"),
            "id = \"t\"\nsource_repo = \"rich\"\n",
        )
        .unwrap();
        assert_eq!(source_repo(&tmp).as_deref(), Some("rich"));
        // Unlabeled task → None (caller falls back to per-task clustering).
        std::fs::write(tmp.join("task.toml"), "id = \"t\"\n").unwrap();
        assert_eq!(source_repo(&tmp), None);
        // Empty label → None.
        std::fs::write(tmp.join("task.toml"), "id = \"t\"\nsource_repo = \"\"\n").unwrap();
        assert_eq!(source_repo(&tmp), None);
        // No task.toml at all → None.
        let _ = std::fs::remove_file(tmp.join("task.toml"));
        assert_eq!(source_repo(&tmp), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tree_digest_detects_tampering() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-test-{}", std::process::id()));
        let tests_dir = tmp.join("tests");
        std::fs::create_dir_all(&tests_dir).unwrap();
        let key = tests_dir.join("expected.json");
        std::fs::write(&key, "{}").unwrap();
        let before = tree_digest(&[&tests_dir]);
        assert_eq!(tree_digest(&[&tests_dir]), before);
        std::fs::write(&key, "{\"defects\": []}").unwrap();
        assert_ne!(tree_digest(&[&tests_dir]), before);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // validate_task_dir
    // -----------------------------------------------------------------------

    #[test]
    fn validate_task_dir_rejects_symlinks() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-sym-{}", std::process::id()));
        let env_dir = tmp.join("environment");
        std::fs::create_dir_all(&env_dir).unwrap();
        let evil = env_dir.join("evil");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", &evil).unwrap();
        #[cfg(unix)]
        {
            let result = validate_task_dir(&tmp);
            assert!(result.is_err(), "expected Err for symlink in fixture");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // validate_arena_for_local_execution
    // -----------------------------------------------------------------------

    #[test]
    fn validate_arena_missing_risk_is_refused() {
        let arena = serde_json::json!({"id": "x", "version": "0.1.0"});
        let err = validate_arena_for_local_execution(&arena).unwrap_err();
        assert!(err.0.contains("missing [risk] metadata"));
        assert!(err.0.contains("Harbor/Docker isolation"));
    }

    #[test]
    fn validate_arena_sensitive_class_is_refused() {
        let arena = serde_json::json!({"risk": {"class": "sensitive", "needs_network": true}});
        let err = validate_arena_for_local_execution(&arena).unwrap_err();
        assert!(err.0.contains("Harbor/Docker isolation"));
        assert!(err.0.contains("class=sensitive"));
        assert!(err.0.contains("network"));
    }

    #[test]
    fn validate_arena_low_class_no_flags_passes() {
        let arena = serde_json::json!({"risk": {"class": "low"}});
        assert!(validate_arena_for_local_execution(&arena).is_ok());
    }

    // -----------------------------------------------------------------------
    // build_pi_cmd
    // -----------------------------------------------------------------------

    #[test]
    fn build_pi_cmd_default_isolation_and_append() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let packet = tmp.join("p.md");
        std::fs::write(&packet, "Review carefully and thoroughly.").unwrap();
        let manifest = tmp.join("c.toml");
        std::fs::write(
            &manifest,
            format!(
                "id = \"x\"\nkind = \"pi\"\nmodel = \"m\"\nprompt_packet = \"{}\"\n",
                packet.display()
            ),
        )
        .unwrap();
        // Use absolute path as repo_root to enable resolution
        let cand = load_candidate(&manifest, &tmp).unwrap();
        let cmd = build_pi_cmd(&cand);
        assert!(cmd.contains(&"--no-skills".to_string()));
        assert!(cmd.contains(&"--no-context-files".to_string()));
        assert!(cmd.contains(&"--append-system-prompt".to_string()));
        assert!(
            !cmd.contains(&"--system-prompt".to_string())
                || cmd
                    .iter()
                    .position(|s| s == "--system-prompt")
                    .map(|i| cmd[i] != "--system-prompt")
                    .unwrap_or(true)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn build_pi_cmd_replace_mode_uses_system_prompt_flag() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-cmd2-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let packet = tmp.join("p.md");
        std::fs::write(&packet, "You are the whole system prompt.").unwrap();
        let skill = tmp.join("skill.md");
        std::fs::write(&skill, "# a pi skill").unwrap();
        let agents = tmp.join("agents.md");
        std::fs::write(&agents, "Repo briefing: run bin/gate before claiming done.").unwrap();
        let manifest = tmp.join("c.toml");
        std::fs::write(
            &manifest,
            format!(
                "id = \"x\"\nkind = \"pi\"\nmodel = \"m\"\n\
prompt_packet = \"{packet}\"\nsystem_prompt_mode = \"replace\"\n\
skills = [\"{skill}\"]\nagents_md = \"{agents}\"\n",
                packet = packet.display(),
                skill = skill.display(),
                agents = agents.display(),
            ),
        )
        .unwrap();
        let cand = load_candidate(&manifest, &tmp).unwrap();
        let cmd = build_pi_cmd(&cand);
        assert!(cmd.contains(&"--skill".to_string()));
        assert!(!cmd.contains(&"--no-skills".to_string()));
        assert!(!cmd.contains(&"--no-context-files".to_string()));
        assert!(cmd.contains(&"--system-prompt".to_string()));
        assert!(!cmd.contains(&"--append-system-prompt".to_string()));
        let workdir = tmp.join("ws");
        std::fs::create_dir_all(&workdir).unwrap();
        prepare_workspace(&cand, &workdir).unwrap();
        let agents_text = std::fs::read_to_string(workdir.join("AGENTS.md")).unwrap();
        assert!(agents_text.contains("bin/gate"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // load_candidate / composition hash
    // -----------------------------------------------------------------------

    #[test]
    fn hash_tracks_agents_md_and_skills_content() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-hash-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let skill = tmp.join("skill.md");
        std::fs::write(&skill, "v1").unwrap();
        let agents = tmp.join("agents.md");
        std::fs::write(&agents, "briefing v1").unwrap();
        let manifest = tmp.join("c.toml");
        std::fs::write(
            &manifest,
            format!(
                "id = \"x\"\nkind = \"pi\"\nmodel = \"m\"\n\
skills = [\"{skill}\"]\nagents_md = \"{agents}\"\n",
                skill = skill.display(),
                agents = agents.display(),
            ),
        )
        .unwrap();
        let h1 = load_candidate(&manifest, &tmp).unwrap()["_hash"]
            .as_str()
            .unwrap()
            .to_string();
        std::fs::write(&skill, "v2").unwrap();
        let h2 = load_candidate(&manifest, &tmp).unwrap()["_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(h1, h2);
        std::fs::write(&skill, "v1").unwrap();
        std::fs::write(&agents, "briefing v2").unwrap();
        let h3 = load_candidate(&manifest, &tmp).unwrap()["_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(h3, h1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn invalid_system_prompt_mode_rejected() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-mode-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let manifest = tmp.join("c.toml");
        std::fs::write(
            &manifest,
            "id = \"x\"\nkind = \"pi\"\nmodel = \"m\"\nsystem_prompt_mode = \"yolo\"\n",
        )
        .unwrap();
        let result = load_candidate(&manifest, &tmp);
        assert!(
            result.is_err(),
            "expected Err for invalid system_prompt_mode"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("system_prompt_mode"), "msg={msg}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn composition_hash_tracks_prompt_packet() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-packet-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let packet = tmp.join("packet.md");
        std::fs::write(&packet, "Review carefully.").unwrap();
        let manifest = tmp.join("cand.toml");
        std::fs::write(
            &manifest,
            format!(
                "id = \"x\"\nkind = \"oneshot\"\nmodel = \"m\"\nprompt_packet = \"{}\"\n",
                packet.display()
            ),
        )
        .unwrap();
        let h1 = load_candidate(&manifest, &tmp).unwrap()["_hash"]
            .as_str()
            .unwrap()
            .to_string();
        std::fs::write(&packet, "Review very carefully.").unwrap();
        let h2 = load_candidate(&manifest, &tmp).unwrap()["_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(h1, h2);
        std::fs::write(&packet, "Review carefully.").unwrap();
        let h3 = load_candidate(&manifest, &tmp).unwrap()["_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(h1, h3);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_candidate_rebases_legacy_absolute_repo_refs() {
        let tmp =
            std::env::temp_dir().join(format!("threshold-run-legacy-abs-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("runs/legacy/packets")).unwrap();
        let packet = tmp.join("runs/legacy/packets/prompt.md");
        std::fs::write(&packet, "Review from a moved checkout.").unwrap();
        let manifest = tmp.join("cand.toml");
        std::fs::write(
            &manifest,
            "id = \"x\"\nkind = \"oneshot\"\nmodel = \"m\"\n\
prompt_packet = \"/old/machine/threshold/runs/legacy/packets/prompt.md\"\n",
        )
        .unwrap();

        let candidate = load_candidate(&manifest, &tmp).unwrap();

        assert_eq!(
            candidate["prompt_packet"].as_str().unwrap(),
            "/old/machine/threshold/runs/legacy/packets/prompt.md"
        );
        assert_eq!(
            candidate["_packet_text"].as_str().unwrap(),
            "Review from a moved checkout."
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // select_tasks
    // -----------------------------------------------------------------------

    fn write_minimal_arena(tmp: &Path, risk_block: &str) -> (PathBuf, Value) {
        let arena_dir = tmp.join("arena");
        let task = arena_dir.join("tasks").join("sample");
        let env = task.join("environment");
        std::fs::create_dir_all(&env).unwrap();
        std::fs::write(env.join("PR.diff"), "diff --git a/x b/x\n").unwrap();
        std::fs::create_dir_all(task.join("tests")).unwrap();
        std::fs::write(
            task.join("tests").join("expected.json"),
            "{\"defects\": []}\n",
        )
        .unwrap();
        std::fs::create_dir_all(task.join("solution")).unwrap();
        std::fs::write(
            task.join("solution").join("findings.json"),
            "{\"findings\": []}\n",
        )
        .unwrap();
        std::fs::write(task.join("instruction.md"), "Review this.").unwrap();
        let toml_content = format!(
            "id = \"sample\"\nversion = \"0.1.0\"\ntaskspec = \"specs/sample/taskspec.toml\"\n\
{risk_block}\n[split]\ntrain = [\"sample\"]\nvalidation = []\nholdout = []\n"
        );
        std::fs::write(arena_dir.join("arena.toml"), &toml_content).unwrap();
        let arena = load_toml(&arena_dir.join("arena.toml")).unwrap();
        (arena_dir, arena)
    }

    #[test]
    fn select_tasks_train_split_returns_matching_tasks() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-select-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (arena_dir, arena) = write_minimal_arena(&tmp, "[risk]\nclass = \"low\"\n");
        let tasks = select_tasks(&arena_dir, &arena, "train", None, false).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].file_name().unwrap().to_str().unwrap(), "sample");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn select_tasks_unknown_split_errors() {
        let tmp =
            std::env::temp_dir().join(format!("threshold-run-badsplit-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (arena_dir, arena) = write_minimal_arena(&tmp, "[risk]\nclass = \"low\"\n");
        let result = select_tasks(&arena_dir, &arena, "nonsense", None, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("nonsense"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn select_tasks_empty_holdout_returns_no_tasks() {
        // holdout = [] → valid, selects nothing
        let tmp =
            std::env::temp_dir().join(format!("threshold-run-holdout-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let (arena_dir, arena) = write_minimal_arena(&tmp, "[risk]\nclass = \"low\"\n");
        let tasks = select_tasks(&arena_dir, &arena, "holdout", None, true).unwrap();
        assert_eq!(tasks.len(), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // task_instruction
    // -----------------------------------------------------------------------

    #[test]
    fn task_instruction_template_replaces_intent() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-instr-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let task_dir = tmp.join("tasks").join("t1");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("intent.md"), "  find the bug  ").unwrap();
        std::fs::write(tmp.join("template.md"), "Review: {intent}\nEnd.").unwrap();
        let arena = serde_json::json!({"template": {"file": "template.md"}});
        let result = task_instruction(&tmp, &arena, &task_dir).unwrap();
        assert_eq!(result, "Review: find the bug\nEnd.");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn task_instruction_falls_back_to_instruction_md() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-instr2-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let task_dir = tmp.join("tasks").join("t1");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("instruction.md"), "Direct instruction.").unwrap();
        let arena = serde_json::json!({"id": "x"});
        let result = task_instruction(&tmp, &arena, &task_dir).unwrap();
        assert_eq!(result, "Direct instruction.");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // summarize
    // -----------------------------------------------------------------------

    #[test]
    fn summarize_basic_aggregates() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-summ-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("trials.jsonl");
        let records = [
            serde_json::json!({"candidate_id": "oracle", "candidate_kind": "oracle", "composition_hash": "abc", "task_id": "t1", "reward": 1.0, "cost_usd": null, "wall_ms": 100, "error": null}),
            serde_json::json!({"candidate_id": "oracle", "candidate_kind": "oracle", "composition_hash": "abc", "task_id": "t2", "reward": 1.0, "cost_usd": null, "wall_ms": 200, "error": null}),
        ];
        let body: String = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();
        let summary = summarize(&path).unwrap();
        let oracle = &summary["oracle"];
        assert_eq!(oracle["trials"].as_i64().unwrap(), 2);
        assert_eq!(oracle["reward_mean"].as_f64().unwrap(), 1.0);
        assert!(!oracle["cost_known"].as_bool().unwrap());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn summarize_task_level_min_max_mean() {
        let tmp = std::env::temp_dir().join(format!("threshold-run-summ2-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("trials.jsonl");
        let records = [
            serde_json::json!({"candidate_id": "a", "candidate_kind": "pi", "composition_hash": "h", "task_id": "t1", "reward": 0.0, "cost_usd": 0.01, "wall_ms": 1000, "error": null}),
            serde_json::json!({"candidate_id": "a", "candidate_kind": "pi", "composition_hash": "h", "task_id": "t1", "reward": 1.0, "cost_usd": 0.01, "wall_ms": 2000, "error": null}),
        ];
        let body: String = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();
        let summary = summarize(&path).unwrap();
        let a = &summary["a"];
        let t1 = &a["tasks"]["t1"];
        assert_eq!(t1["min"].as_f64().unwrap(), 0.0);
        assert_eq!(t1["max"].as_f64().unwrap(), 1.0);
        assert_eq!(t1["mean"].as_f64().unwrap(), 0.5);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // run_with_timeout (Fix A: per-trial subprocess timeout)
    // -----------------------------------------------------------------------

    #[test]
    fn run_with_timeout_kills_a_hanging_child() {
        // `exec` so sh REPLACES itself with sleep — no forked grandchild holds
        // the pipe, so killing the child closes it immediately and this exercises
        // the clean kill+drain path deterministically (the grandchild path is
        // covered by the test below). Plain `sleep 30` makes dash fork a
        // grandchild that keeps the pipe open → multi-second drain, flaky on Linux CI.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "exec sleep 30"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let t0 = std::time::Instant::now();
        let (status, _out, _err) =
            run_with_timeout(cmd, std::time::Duration::from_millis(500)).unwrap();
        assert!(
            status.is_none(),
            "a hanging child must be killed (None status)"
        );
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(5),
            "should return promptly after the kill, took {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn run_with_timeout_captures_a_fast_child() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "printf hello; printf oops 1>&2; exit 3"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let (status, out, err) = run_with_timeout(cmd, std::time::Duration::from_secs(10)).unwrap();
        let status = status.expect("a fast child should exit before the deadline");
        let out = String::from_utf8_lossy(&out).into_owned();
        let err = String::from_utf8_lossy(&err).into_owned();
        assert_eq!(status.code(), Some(3));
        assert_eq!(out, "hello");
        assert_eq!(err, "oops");
    }

    #[test]
    fn run_with_timeout_drains_output_larger_than_a_pipe_buffer() {
        // 200KB > the ~64KB OS pipe buffer: without background draining the
        // child would deadlock, and a clean exit must never truncate output.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "head -c 200000 /dev/zero"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let (status, out, _err) =
            run_with_timeout(cmd, std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(status.expect("should exit cleanly").code(), Some(0));
        assert_eq!(
            out.len(),
            200_000,
            "all output must be captured, not truncated"
        );
    }

    #[test]
    fn run_with_timeout_does_not_hang_on_a_pipe_holding_grandchild() {
        // The parent exits 0 immediately but backgrounds a grandchild that
        // inherits and holds stdout for 20s. Collection must return within the
        // drain grace (~3s), not block on the grandchild's EOF for 20s. With a
        // blocking recv() on the clean-exit path this would hang past 20s.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "(sleep 20 &) ; exit 0"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let t0 = std::time::Instant::now();
        let (status, _out, _err) =
            run_with_timeout(cmd, std::time::Duration::from_secs(60)).unwrap();
        assert_eq!(status.expect("parent exits cleanly").code(), Some(0));
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(10),
            "must not hang on the grandchild's pipe, took {:?}",
            t0.elapsed()
        );
    }

    // -----------------------------------------------------------------------
    // oneshot_context_fits (Fix B: skip one-shot probe on context overflow)
    // -----------------------------------------------------------------------

    #[cfg(feature = "http")]
    #[test]
    fn oneshot_context_fits_rejects_real_repo_workspace() {
        // pr-review-v2's py-export-clear dumps ~1.4M chars (~350k tokens),
        // which overflows a 256k window — the exact bug this guards against.
        assert!(oneshot_context_fits(1_400_000, 8192, 256_000).is_err());
    }

    #[cfg(feature = "http")]
    #[test]
    fn oneshot_context_fits_allows_small_fixture() {
        // v0/v1 synthetic fixtures are a few KB and must still be probed.
        assert!(oneshot_context_fits(8_000, 8192, 256_000).is_ok());
    }

    #[cfg(feature = "http")]
    #[test]
    fn oneshot_context_fits_counts_the_completion_budget() {
        // Prompt alone fits; prompt + max_tokens tips it over the window.
        let prompt_chars = 99_000 * 4; // ~99k prompt tokens
        assert!(oneshot_context_fits(prompt_chars, 500, 100_000).is_ok());
        assert!(oneshot_context_fits(prompt_chars, 5_000, 100_000).is_err());
    }

    #[cfg(feature = "http")]
    #[test]
    fn oneshot_context_fits_rounds_partial_tokens_up() {
        // One char past an exact token boundary counts as a full token: a
        // prompt of context_window*4 + 1 chars (no completion) must be rejected,
        // not admitted by floor division.
        assert!(oneshot_context_fits(256_000 * 4 + 1, 0, 256_000).is_err());
        assert!(oneshot_context_fits(256_000 * 4, 0, 256_000).is_ok());
    }

    #[test]
    fn review_context_probe_includes_diff_and_changed_file_not_full_repo() {
        let tmp = std::env::temp_dir().join(format!(
            "threshold-run-review-context-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let task = tmp.join("task");
        let workdir = tmp.join("workdir");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::create_dir_all(workdir.join("src")).unwrap();
        std::fs::write(task.join("intent.md"), "Fix changed path only").unwrap();
        std::fs::write(
            workdir.join("PR.diff"),
            "diff --git a/src/ratio.py b/src/ratio.py\n--- a/src/ratio.py\n+++ b/src/ratio.py\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        std::fs::write(workdir.join("src/ratio.py"), "def ratio():\n    return 0\n").unwrap();
        std::fs::write(workdir.join("src/unrelated.py"), "UNRELATED\n".repeat(2000)).unwrap();

        let candidate = serde_json::json!({
            "workspace_mode": "review-context",
            "workspace_max_bytes": 4096,
            "workspace_file_bytes": 1024
        })
        .as_object()
        .unwrap()
        .clone();
        let context = oneshot_workspace_context(&candidate, &task, &workdir).unwrap();

        assert!(context.text.contains("Fix changed path only"));
        assert!(context.text.contains("### PR.diff"));
        assert!(context.text.contains("### src/ratio.py"));
        assert!(!context.text.contains("src/unrelated.py"));
        assert_eq!(context.files, vec!["PR.diff", "src/ratio.py"]);
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn review_context_probe_without_diff_falls_back_to_full_workspace() {
        let tmp = std::env::temp_dir().join(format!(
            "threshold-run-review-context-no-diff-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let task = tmp.join("task");
        let workdir = tmp.join("workdir");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::create_dir_all(workdir.join("src")).unwrap();
        std::fs::write(task.join("intent.md"), "Small fixture without PR diff").unwrap();
        std::fs::write(workdir.join("src/main.py"), "print('covered')\n").unwrap();

        let candidate = serde_json::json!({
            "workspace_mode": "review-context",
            "workspace_max_bytes": 96,
            "workspace_file_bytes": 64
        })
        .as_object()
        .unwrap()
        .clone();
        let context = oneshot_workspace_context(&candidate, &task, &workdir).unwrap();

        assert!(context.text.contains("src/main.py"));
        assert!(context.text.contains("print('covered')"));
        assert_eq!(context.files, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn changed_paths_from_diff_handles_metadata_and_quoted_paths() {
        let diff = "\
diff --git a/src/plain.py b/src/plain.py
--- a/src/plain.py
+++ b/src/plain.py\t2026-06-20
diff --git a/src/ratio with spaces.py b/src/ratio with spaces.py
--- \"a/src/ratio with spaces.py\"
+++ \"b/src/ratio with spaces.py\"
diff --git a/src/deleted.py b/src/deleted.py
--- a/src/deleted.py
+++ /dev/null
diff --git a/secret b/secret
--- a/secret
+++ b/../secret
";

        assert_eq!(
            changed_paths_from_diff(diff),
            vec!["src/plain.py", "src/ratio with spaces.py"]
        );
    }

    #[test]
    fn push_text_section_enforces_byte_limits_on_utf8_boundaries() {
        let mut out = String::new();
        let mut truncated = false;

        assert!(push_text_section(
            &mut out,
            "unicode",
            &"é".repeat(100),
            40,
            64,
            &mut truncated,
        ));

        assert!(truncated);
        assert!(out.len() <= 64);
        assert!(out.contains("...<truncated>"));
    }

    #[test]
    fn review_context_probe_keeps_truncation_notice_within_byte_limit() {
        let tmp = std::env::temp_dir().join(format!(
            "threshold-run-review-context-limit-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let task = tmp.join("task");
        let workdir = tmp.join("workdir");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::create_dir_all(workdir.join("src")).unwrap();
        std::fs::write(task.join("intent.md"), "é".repeat(200)).unwrap();
        std::fs::write(
            workdir.join("PR.diff"),
            "diff --git a/src/main.py b/src/main.py\n--- a/src/main.py\n+++ b/src/main.py\n",
        )
        .unwrap();
        std::fs::write(workdir.join("src/main.py"), "print('new')\n").unwrap();

        let candidate = serde_json::json!({
            "workspace_mode": "review-context",
            "workspace_max_bytes": 96,
            "workspace_file_bytes": 64
        })
        .as_object()
        .unwrap()
        .clone();
        let context = oneshot_workspace_context(&candidate, &task, &workdir).unwrap();

        assert!(context.truncated);
        assert!(context.text.len() <= 96);
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[cfg(feature = "http")]
    #[test]
    fn review_context_probe_can_fit_when_full_workspace_would_overflow() {
        let tmp = std::env::temp_dir().join(format!(
            "threshold-run-review-context-fit-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let task = tmp.join("task");
        let workdir = tmp.join("workdir");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::create_dir_all(workdir.join("src")).unwrap();
        std::fs::write(task.join("intent.md"), "Review bounded context").unwrap();
        std::fs::write(
            workdir.join("PR.diff"),
            "diff --git a/src/main.py b/src/main.py\n--- a/src/main.py\n+++ b/src/main.py\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        std::fs::write(workdir.join("src/main.py"), "print('new')\n").unwrap();
        std::fs::write(workdir.join("huge.txt"), "x".repeat(1_400_000)).unwrap();

        let full = workspace_listing(&workdir);
        assert!(oneshot_context_fits(full.chars().count(), 8192, 256_000).is_err());

        let candidate = serde_json::json!({
            "workspace_mode": "review-context",
            "workspace_max_bytes": 4096,
            "workspace_file_bytes": 1024
        })
        .as_object()
        .unwrap()
        .clone();
        let context = oneshot_workspace_context(&candidate, &task, &workdir).unwrap();
        assert!(oneshot_context_fits(context.text.chars().count(), 8192, 256_000).is_ok());
        let _ = std::fs::remove_dir_all(tmp);
    }
}
