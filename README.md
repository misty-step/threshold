# Threshold

Threshold is a laboratory for an agent that builds agents.

## Current Factory Posture (2026-07-01)

Threshold is parked behind Crucible. The optimizer remains strategically
important, but optimization should not resume until Crucible can run and grade
benchmarks for real, Sprite candidate artifacts are scored against arena answer
keys, and the duplicate optimizer stacks collapse into one typed loop.

The 2026-07-01 optimizer-loop run at
`runs/20260701T182031Z-optimizer-loop-pr-review-key-recall-v0/` is a plumbing
proof only. It proves the Crucible target -> Threshold optimizer -> Bitter
Blossom Sprite receipt path, not candidate quality. Its score formula was
`source_split_key_recall * remote_verdict_score`, so the candidate-dependent
signal was remote self-verdict rather than answer-key grading. See
`backlog.d/066-real-sprite-grading-and-one-optimizer.md` for the reentry gate.

The working idea is not a general-purpose "make me an agent" button. It is a
controlled research program: start with a concrete task specification, have a
frontier master agent design the evaluation surface and sandbox, then search
over focused agent compositions until a candidate is measurably good enough to
hand to a human or a downstream runtime.

If this works, Threshold can become a way to define, generate, maintain, and
extend bespoke agents across several contexts: ad hoc local runs, repo-specific
harnesses, Olympus-style operator workflows, Bitter Blossom-style product
surfaces, or a future agent control plane. This repository starts as a seed
document only. It is intentionally not yet a product spec, implementation plan,
or runtime contract.

## Intent

The project explores whether a strong master agent can turn a task into a
focused agent by doing the hard setup work that humans currently do by hand:

- clarifying the task and its tradeoffs;
- designing task-specific evals and benchmarks;
- constructing an isolated sandbox, arena, or fixture set;
- choosing candidate models, providers, harnesses, tools, skills, prompts, and
  runtime constraints;
- running candidate agents against the arena;
- logging tokens, latency, cost, traces, scores, and artifacts;
- sanity-checking whether the eval actually measures the intended behavior;
- iterating in an autoresearch loop until a budget, score, or turn limit is
  reached;
- emitting a candidate launch contract that a human or downstream system can
  review.

The guiding question is simple:

> Can a master agent reliably convert a task specification into a cheaper,
> faster, narrower, better-tested focused agent?

## Background

Several current patterns point in the same direction.

[Karpathy's autoresearch](https://github.com/karpathy/autoresearch) is the
clearest operational inspiration. It fixes the environment and evaluator,
lets the agent mutate only the training script, runs every experiment for a
fixed time budget, scores against validation bits per byte, then keeps or
discards the change. The important lesson is not "AI does research while you
sleep." The important lesson is the shape of the loop:

1. Fix the arena.
2. Name the mutable surface.
3. Run a bounded experiment.
4. Score with a stable metric.
5. Keep, discard, or learn.
6. Repeat with a trace.

Threshold asks what the equivalent loop looks like for agents. In this project,
the mutable surface is not a training script. It might be an agent manifest, prompt
packet, skill list, tool policy, model/provider choice, or harness projection.
The fixed arena is not a small language-model training run. It might be a pull
request review fixture, a backlog grooming corpus, a synthetic inbox, a browser
task environment, or a product operations workflow.

The rest of the research landscape reinforces a few constraints:

- [OpenAI's eval guidance](https://developers.openai.com/api/docs/guides/evaluation-best-practices)
  treats evals as structured tests for nondeterministic systems and emphasizes
  task-specific tests, logging, automation, and human calibration. The same
  docs now warn that OpenAI's Evals platform is being deprecated, so Threshold
  should not assume that platform as its durable substrate.
- [Anthropic's agent eval guidance](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents)
  frames agent evals as multi-turn tasks with tools, environments, and grading
  over what changed in the world.
- [Inspect AI](https://inspect.aisi.org.uk/) provides a useful mental model:
  datasets, agents, tools, and scorers should be composable pieces, not a
  monolith.
- [LangSmith trajectory evals](https://docs.langchain.com/langsmith/trajectory-evals)
  separate final-answer grading, trajectory grading, and step-level grading.
  Threshold should make that distinction explicit instead of collapsing every
  signal into a vague score.
- [DSPy GEPA optimization](https://dspy.ai/getting-started/gepa-optimization/)
  and the [GEPA paper](https://arxiv.org/abs/2507.19457) suggest that
  reflective prompt/workflow evolution can be more sample-efficient than blind
  sweeps when the loop can inspect failures and preserve a Pareto frontier.
- [SWE-bench Verified](https://www.swebench.com/verified.html) and
  [OSWorld](https://os-world.github.io/) show why real environments, reset
  scripts, reproducible fixtures, and execution-based grading matter.
- Vercel's
  [agent eval work](https://github.com/vercel-labs/agent-eval) and
  [`AGENTS.md` eval writeup](https://vercel.com/blog/agents-md-outperforms-skills-in-our-agent-evals)
  are a useful warning: sophisticated retrieval or skill systems do not
  automatically beat small, always-visible, version-matched context. Threshold
  should test boring baselines.

## Core Thesis

The master agent should not produce a free-form pile of prose that says "use
Claude with a code-review prompt." It should produce a measured candidate
agent package:

- a task contract;
- an arena;
- an eval suite;
- one or more candidate compositions;
- run records with traces and resource accounting;
- a meta-eval report that checks whether the eval is meaningful;
- a recommended launch contract with explicit runtime triggers and guardrails.

The output should be closer to a capability capsule or launch contract than to
runtime-generated global harness prose. Durable skills and project doctrine can
be promoted later, after repeated evidence. The experimental loop should not
silently mutate global agent instructions, shared provider state, or production
runtime triggers.

## The Task Specification

Everything begins with a task specification. The master agent's first job is to
make the specification sharp enough to test.

A useful task spec probably needs:

- **Goal:** the exact work the focused agent is supposed to do.
- **Domain:** code review, backlog grooming, inbox triage, calendar ops,
  browser QA, research synthesis, product shaping, or another bounded domain.
- **Inputs:** files, PRs, commits, issues, emails, tickets, browser states,
  API data, screenshots, or human-provided packets.
- **Output contract:** review findings, backlog edits, draft replies, run
  reports, structured JSON, artifacts, commits, or recommendations.
- **Acceptance oracle:** what makes the output correct enough.
- **Negative examples:** what a bad output looks like.
- **Risk class:** secrets, money movement, production writes, reputational
  risk, legal/medical/financial sensitivity, or low-risk local work.
- **Budget posture:** maximize quality, minimize latency, minimize cost after a
  quality threshold, or maintain a Pareto frontier.
- **Runtime trigger:** manual run, cron, webhook, PR event, inbox threshold,
  queue item, scheduled brief, or downstream app event.
- **Human checkpoint:** what must be reviewed before deployment or action.
- **Data boundaries:** which fixtures, repos, inboxes, browsers, APIs, and
  credentials the experiment may use.

The first hard product question is whether Threshold should require this task
spec as a structured document from the user, or whether the master agent should
derive it through a clarifying interview.

The likely answer is both: accept a structured packet when available; otherwise
run a short clarification pass before any expensive search begins.

## Quality, Cost, and Latency Modes

Not every agent should optimize the same objective.

Some likely modes:

- **Max quality:** use expensive models, many critics, broad context, and
  deeper search. Cost and latency are secondary.
- **Threshold then cheap:** hit a minimum quality bar, then minimize cost.
- **Fast enough:** optimize for latency with a quality floor.
- **Pareto frontier:** preserve several candidates across quality, cost, and
  latency instead of collapsing to one scalar.
- **Conservative:** optimize for low false-positive or low false-negative
  rates, depending on the domain.
- **Human-assist:** optimize for the best artifact to hand to a human, not for
  autonomous completion.

The task spec should say which mode applies. If it does not, the master agent
should ask before running the search.

## Master Agent Responsibilities

The master agent is the expensive, high-reasoning coordinator. It is allowed to
think broadly, research, design experiments, and decide what to try. It should
not be the cheap worker that runs every candidate task.

Its responsibilities are:

1. **Clarify the task.** Convert an ambiguous request into a testable task
   specification.
2. **Define the arena.** Decide what fixture set, sandbox, worktree, container,
   browser state, inbox sample, or API simulator the candidates will use.
3. **Design evals.** Build the scoring surface before optimizing the candidate
   agent.
4. **Freeze boundaries.** Declare which files, prompts, tools, models, and
   graders are immutable for a run.
5. **Generate candidates.** Propose focused agent compositions.
6. **Run experiments.** Execute candidates under comparable budgets.
7. **Collect traces.** Persist model ids, harness ids, prompts, tools, tokens,
   cost, latency, stdout/stderr, artifacts, and scorer output.
8. **Evaluate the eval.** Check whether the scorer rewards the intended
   behavior, catches obvious failures, and agrees with held-out human judgment.
9. **Reflect and iterate.** Use failures to propose the next candidate rather
   than randomly sweeping the whole space.
10. **Recommend promotion.** Emit a launch contract and residual risks, not an
    automatic production deployment.

## Focused Agent Composition

A candidate focused agent should be represented as a typed composition, not as
a blob of instructions.

Possible slots:

- model id;
- provider and harness;
- reasoning budget;
- prompt packet;
- task-specific skill set;
- tool/MCP allowlist;
- filesystem or browser permissions;
- retrieval context;
- planner/executor/critic split;
- maximum turns;
- maximum wall-clock time;
- token and cost ceilings;
- required artifacts;
- escalation triggers;
- output schema.

The point of slots is experimental discipline. If two candidates differ in
model, prompt, tool surface, and critic count at once, it is hard to know what
caused a score change. Threshold should support broad creative proposals, but
the experiment runner should preserve enough structure for ablation.

## Arena and Sandbox

The arena is the reality layer against which candidates are tested.

For a code-review agent, the arena might include:

- frozen PR diffs;
- repo snapshots;
- known defects;
- expected findings;
- hidden regression cases;
- a rubric for severity and actionability.

For backlog grooming:

- frozen backlog slices;
- product strategy notes;
- historical examples of good and bad grooming;
- expected archive, split, merge, or clarify actions.

For inbox processing:

- synthetic or redacted email threads;
- known reply/no-reply labels;
- priority labels;
- escalation examples;
- safety cases around confidential or sensitive content.

The arena should be reset between runs, versioned, and isolated. Candidate
agents should not be able to edit the grader, fixture set, or hidden answer
keys. If they can, the loop will eventually optimize the benchmark instead of
the behavior.

## Evals and Benchmarks

Threshold should treat eval design as the first-class product.

Useful eval types:

- **Deterministic checks:** tests, schema validation, expected files, expected
  diffs, API state, browser state, or fixture assertions.
- **Outcome grading:** did the final artifact solve the task?
- **Trajectory grading:** did the agent use the expected tools, avoid forbidden
  tools, recover from failure, or inspect required evidence?
- **Step grading:** did it choose the right next action at critical points?
- **LLM-as-judge grading:** useful for subjective outputs, but only after
  calibration against human-labeled examples.
- **Pairwise comparisons:** candidate A vs candidate B under the same fixture.
- **Budget grading:** quality per dollar, quality per minute, or quality under
  a fixed ceiling.
- **Safety grading:** secret handling, dual-use behavior, write-action
  restraint, and escalation behavior.
- **Regression grading:** does a new candidate preserve behavior on old task
  fixtures?

The score that drives keep/discard should be explicit. Other signals can be
diagnostics, but a hidden soup of metrics is not an objective function.

## Meta-Evals

The project should assume that generated evals are suspect until proven
otherwise.

A meta-eval asks whether the eval is evaluating the intended behavior. Examples:

- Can a bad candidate pass by formatting the output correctly?
- Does the scorer reward long, confident prose over correct action?
- Does the judge prefer the model family that generated the answer?
- Do two independent judges agree?
- Does the eval catch known bad examples?
- Does it overfit to visible fixture names?
- Does a cheap baseline expose that the benchmark is too easy?
- Does a human-labeled holdout set agree with the automated score?

This is where the master agent should be adversarial. The embarrassing failure
mode is not a low-scoring candidate. The embarrassing failure mode is a
high-scoring candidate that is obviously bad to a human.

## The Autoresearch Loop

The simplest Threshold loop should look like this:

1. Load the task spec.
2. Create or select a fixed arena.
3. Create or select a fixed eval suite.
4. Run a cheap baseline.
5. Generate one candidate composition.
6. Run it under a fixed budget.
7. Score it.
8. Record the full trace.
9. Keep it if it improves the declared objective.
10. Reflect on the failure or success.
11. Propose the next candidate.
12. Stop at a turn cap, budget cap, plateau, or score threshold.

The first implementation should probably avoid a full multi-objective optimizer.
Start with one domain, one mutable surface, and one primary scalar at a fixed
budget. Add Pareto search after the basic loop is real.

## Observability

Every run should leave a replayable record.

Minimum run record fields:

- task spec id;
- arena id and version;
- eval suite id and version;
- candidate id;
- model/provider/harness ids;
- prompt and skill hashes;
- tool allowlist;
- permissions profile;
- random seed or fixture seed when applicable;
- start/end timestamps;
- wall-clock duration;
- token counts by provider category when available;
- estimated and provider-reported cost;
- latency by phase;
- tool-call trace;
- stdout/stderr or transcript references;
- generated artifacts;
- scorer output;
- meta-eval output;
- keep/discard decision;
- lead verdict;
- residual risks.

Cost and latency are not secondary logs. They are part of the search objective.
A candidate that scores 3 percent higher at 10x cost may be right for max-quality
mode and wrong for a threshold-then-cheap mode.

## Runtime Triggers

The master agent should define runtime triggers, but it should not silently
deploy them.

Possible trigger classes:

- manual command;
- cron schedule;
- GitHub PR or commit event;
- issue or backlog item update;
- inbox arrival or inbox threshold;
- calendar event;
- queue message;
- webhook from an application;
- local file or folder change;
- human approval after a recommended run.

The output of Threshold should be a recommended trigger contract:

- what event starts the agent;
- what input packet is built;
- what permissions are granted;
- what budget applies;
- what outputs are allowed;
- what actions require human approval;
- how failures are retried or escalated;
- where traces and receipts are stored.

Offline benchmark success should not automatically become production runtime
permission.

## Example Candidate Tasks

These are example domains, not commitments.

### Pull Request Review

Given a PR or commit, create a focused review agent that finds bugs, behavioral
regressions, security issues, missing tests, and repo-fit problems. The arena
could use historical PRs with known review findings, seeded bugs, and hidden
fixtures. Evals can score finding accuracy, severity calibration, false-positive
rate, evidence quality, and whether the agent avoids broad style churn.

### Backlog Grooming

Given a backlog slice and project context, produce archive/split/merge/clarify
recommendations. Evals can compare against human-curated grooming decisions,
measure whether the agent preserves strategic intent, and check that it does
not close work from insufficient evidence.

### Inbox Processing

Given an inbox sample, classify urgency, draft replies, identify waiting states,
and escalate sensitive messages. Evals can use redacted historical mail,
synthetic adversarial messages, priority labels, and human-review agreement.
Safety boundaries matter here: no sending, forwarding, or mutating state until
the runtime contract earns that permission.

### Agent Control Plane Support

Given a product surface like Olympus or Bitter Blossom, generate focused agents
that operate behind explicit contracts. The output should be importable into
the product, but the product should own live scheduling, permissions, and user
trust boundaries.

## Safety and Governance

Agent generation has obvious misuse modes. A system that can produce focused
agents for inboxes, social workflows, PR reviews, or product operations can also
produce spam, manipulation, noisy review bots, unsafe automations, and brittle
agents with too much authority.

Standing constraints:

- No candidate gets write access to secrets, graders, hidden answer keys, or
  live user data unless the task explicitly requires it and the sandbox is safe.
- No offline winner promotes itself to production.
- No generated agent gets runtime authority without a human-readable launch
  contract.
- No eval score is trusted without calibration or adversarial sanity checks.
- No global harness prose is mutated during the experiment loop.
- No agent is judged only by its own narrative.
- No cost or latency claim is accepted without recorded usage evidence or a
  stated "unknown."

## Non-Goals for the First Version

- Do not build a universal agent marketplace.
- Do not build a production scheduler first.
- Do not auto-generate durable global skills during the loop.
- Do not start with every domain at once.
- Do not let candidate agents edit their graders.
- Do not make LLM-as-judge the only oracle.
- Do not optimize prompts without frozen examples.
- Do not choose a provider default from vibes.
- Do not confuse "agent ran" with "agent is useful."

## Likely First Experiment

The first real experiment should be narrow:

- one task family;
- one frozen fixture set;
- one immutable scorer;
- one cheap baseline;
- one expensive baseline;
- one mutable surface;
- one fixed budget;
- one replayable trace format;
- one human review pass over the eval quality.

Pull-request review is a strong candidate because the artifacts are concrete:
diffs, files, tests, known defects, review findings, and severity labels. It
also has obvious product value and clear failure modes.

Inbox processing may be more product-relevant, but it introduces sensitive data
and action-safety concerns earlier. Backlog grooming is strategically useful,
but its success criteria are harder to grade without a strong human-labeled
corpus.

## Naming

Name: **Threshold** (renamed from the working name Daedalus on 2026-07-01).

Why it fits:

- The whole project is a search for the configuration that clears a bar — the
  threshold of acceptable score at acceptable cost — not a search for the
  single best config in the abstract. "Threshold" names that objective
  directly.
- It reads as plain and mechanical rather than mythic, which fits a Rust CLI
  that produces evidence-backed launch contracts, not a persona.
- It sits cleanly next to **Crucible**, the sibling product that owns eval
  design and measurement: Crucible defines what "clears the bar" means,
  Threshold searches for configs that clear it. See [The Split With
  Crucible](VISION.md#the-split-with-crucible).

Other candidates considered before the rename:

- **Ariadne:** emphasizes guidance, thread, traceability, and escape from the
  maze. Strong if the project becomes more about audit trails than generation.
- **Atelier:** emphasizes bespoke craft. Good fit for custom agents, less
  technical.
- **Kiln:** compact, material, experimental. Good if the project centers on
  turning raw task specs into hardened artifacts.
- **Caliper:** measurement-first, restrained, precise. Better for an eval
  subsystem than the whole project.
- **Hephaestus:** forge-builder energy, but heavier and less clean than
  Threshold.
- **Labyrinth:** honest about the arena concept, but too much risk of sounding
  like complexity is the product.
- **Whetstone:** excellent sharpening metaphor, but already present locally and
  more "improve existing edge" than "design a new agent."
- **Scry:** fits eval/oracle/search, but already present locally.
- **Glyph:** nice for compact agent recipes, but already present locally.
- **Laboratory:** radically honest, but already present locally and too broad.

## Open Questions

- What is the first task family: PR review, backlog grooming, inbox processing,
  or something else?
- Is the task specification a file format, an interview protocol, or both?
- What is the minimum viable arena format?
- Which eval substrate should be used first: custom runner, Inspect AI, unit tests,
  LangSmith-style trajectory checks, or something else?
- How should model/provider pricing be normalized when provider usage reports
  are incomplete?
- What human-labeled corpus is available for the first task family?
- How much should the master agent be allowed to research externally during a
  run?
- How should candidate launch contracts be imported into Olympus, Bitter
  Blossom, or ad hoc harnesses?
- What is the promotion path from experimental candidate to durable skill,
  repo doctrine, or runtime agent?
- Where is the line between agent generation and agent governance?

## Implementation

Threshold is implemented in **Rust**: `crates/threshold-core` (the deterministic
kernel) and `crates/threshold-cli` (the `threshold` binary). It was migrated from
the initial Python prototype one parity-verified module at a time — every port
was cross-checked against the Python reference by a parity oracle before the
Python was retired; see `docs/rust-migration.md`.

Harbor task containers run the copied Rust `threshold-score` verifier binary
inside the Harbor isolation image. `bin/harbor-run` builds `threshold-score` and
calls the Rust `port-harbor` subcommand before invoking Harbor. Shell stays a
thin launcher (`bin/gate`, `bin/harbor-run`), never the workflow engine.

The core stays small: task specs in, experiments out, receipts persisted. The
master agent can be clever. The harness should be boring.

## Current Status

Seed repository initialized on 2026-06-09. The first task family (PR review)
and acceptance oracle (deterministic seeded-defect keys) were chosen the same
day, unlocking the Phase 0 prototype:

- `DESIGN.md` — architecture, file contracts, decisions and reopen triggers.
- `ROADMAP.md` — phases 0–4 with evidence-based exit criteria.
- `docs/operator-sop.md` — the maintained cold-start sequence for spec,
  arena validation, certified runs, export, approvals, trace, and closeout.
- `docs/arena-workbench.md` — task scaffold, freeze validation,
  adjudication, and calibration commands.
- `docs/security-posture.md` — local-run risk gates, Harbor/Docker boundary,
  launch-contract validation, and residual risks.
- `specs/pr-review/` — first task specification (gate G1 approved).
- `arenas/pr-review-v0/` — six PR fixtures in Harbor task format.
- `crates/` — the Rust implementation: `threshold-core` (kernel) +
  `threshold-cli` (the `threshold` binary).
- `candidates/` — reference candidates (null floor, oracle ceiling, one-shot
  saturation probe) plus agent compositions (pi over OpenRouter).
- `runs/` — JSONL run records with reward, tokens, cost, and latency.
- `.agents/skills/threshold/SKILL.md` — the master-agent operating protocol.

### Quickstart

The full operator sequence is maintained in `docs/operator-sop.md`. Keep that
file as the source of truth for spec, validation, run, export, approval, trace,
and closeout commands.

```sh
bin/gate                                           # offline gate (cargo fmt --check + cargo test + cargo clippy)
cargo build -p threshold-cli                        # build the `threshold` binary
cargo run --quiet --bin threshold -- doctor         # readiness summary, no model spend
cargo run --quiet --bin threshold -- arena-freeze --help  # generate freeze packets before search
cargo run --quiet --bin threshold -- --help         # all subcommands
```
