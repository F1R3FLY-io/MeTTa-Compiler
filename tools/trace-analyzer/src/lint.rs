//! Lint-style diagnostic passes over MeTTaTron trace files.
//!
//! Runs 8 focused checks in a single sweep over the event stream, detecting
//! parallelism anti-patterns, GC pressure anomalies, tier instability, and
//! missed optimization opportunities.

use std::collections::HashMap;

use trace_format::{TraceEventKind, TraceTier};

use crate::reader::TraceReader;

// ── Severity & Finding ──────────────────────────────────────────────────────

/// Minimum reportable severity level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "info" => Ok(Severity::Info),
            "warning" | "warn" => Ok(Severity::Warning),
            other => Err(format!("unknown severity: {other:?} (expected \"info\" or \"warning\")")),
        }
    }
}

/// A single diagnostic finding emitted by a lint pass.
pub struct Finding {
    pub severity: Severity,
    pub lint_id: &'static str,
    pub message: String,
    /// Optional time range for the finding.
    pub time_range: Option<(u64, Option<u64>)>,
}

impl Finding {
    fn new(severity: Severity, lint_id: &'static str, message: String) -> Self {
        Self { severity, lint_id, message, time_range: None }
    }

    fn with_time(mut self, start_ns: u64, end_ns: Option<u64>) -> Self {
        self.time_range = Some((start_ns, end_ns));
        self
    }
}

fn format_ms(ns: u64) -> String {
    let ms = ns as f64 / 1_000_000.0;
    if ms < 1.0 {
        format!("{ms:.3}ms")
    } else if ms < 100.0 {
        format!("{ms:.2}ms")
    } else {
        format!("{ms:.1}ms")
    }
}

// ── Lint IDs ────────────────────────────────────────────────────────────────

pub const LINT_IDS: &[&str] = &[
    "branch-imbalance",
    "gc-storm",
    "tier-thrash",
    "workpool-saturation",
    "sequential-fan-out",
    "gc-pause-outlier",
    "eval-depth-explosion",
    "compilation-latency",
];

// ── Configuration ───────────────────────────────────────────────────────────

pub struct LintConfig {
    pub min_severity: Severity,
    pub enabled_lints: Option<Vec<String>>,
    pub depth_threshold: u32,
}

impl LintConfig {
    fn is_enabled(&self, lint_id: &str) -> bool {
        match &self.enabled_lints {
            None => true,
            Some(ids) => ids.iter().any(|id| id == lint_id),
        }
    }
}

// ── A. branch-imbalance ─────────────────────────────────────────────────────

struct ForkRecord {
    timestamp_ns: u64,
    thread_id: u32,
    branch_count: u32,
}

struct BranchEndRecord {
    timestamp_ns: u64,
    thread_id: u32,
    duration_ns: Option<u64>,
    branch_index: u32,
}

struct BranchImbalanceAcc {
    forks: Vec<ForkRecord>,
    branch_ends: Vec<BranchEndRecord>,
}

impl BranchImbalanceAcc {
    fn new() -> Self {
        Self { forks: Vec::new(), branch_ends: Vec::new() }
    }

    fn record_fork(&mut self, timestamp_ns: u64, thread_id: u32, branch_count: u32) {
        self.forks.push(ForkRecord { timestamp_ns, thread_id, branch_count });
    }

    fn record_branch_end(
        &mut self,
        timestamp_ns: u64,
        thread_id: u32,
        duration_ns: Option<u64>,
        branch_index: u32,
    ) {
        self.branch_ends.push(BranchEndRecord { timestamp_ns, thread_id, duration_ns, branch_index });
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Build per-thread fork lists sorted by timestamp for binary search
        let mut forks_by_thread: HashMap<u32, Vec<&ForkRecord>> = HashMap::new();
        for fork in &self.forks {
            forks_by_thread.entry(fork.thread_id).or_default().push(fork);
        }
        for forks in forks_by_thread.values_mut() {
            forks.sort_by_key(|f| f.timestamp_ns);
        }

        // Group branch ends to their nearest preceding fork on the same thread
        // Key: (thread_id, fork_timestamp) → Vec<BranchEndRecord>
        let mut fork_groups: HashMap<(u32, u64), (u32, Vec<&BranchEndRecord>)> = HashMap::new();

        for be in &self.branch_ends {
            if let Some(forks) = forks_by_thread.get(&be.thread_id) {
                // Find the nearest preceding fork via binary search
                let idx = forks.partition_point(|f| f.timestamp_ns <= be.timestamp_ns);
                if idx > 0 {
                    let fork = forks[idx - 1];
                    let key = (be.thread_id, fork.timestamp_ns);
                    let entry = fork_groups.entry(key).or_insert_with(|| (fork.branch_count, Vec::new()));
                    entry.1.push(be);
                }
            }
        }

        // Analyze each fork group
        for ((_, fork_ts), (branch_count, branches)) in &fork_groups {
            if branches.len() < 2 {
                continue;
            }

            // Collect durations — skip branches without duration
            let durations: Vec<(u32, u64)> = branches.iter()
                .filter_map(|b| b.duration_ns.map(|d| (b.branch_index, d)))
                .collect();

            if durations.len() < 2 {
                continue;
            }

            // Compute median
            let mut sorted_durs: Vec<u64> = durations.iter().map(|(_, d)| *d).collect();
            sorted_durs.sort_unstable();
            let median = sorted_durs[sorted_durs.len() / 2];

            if median == 0 {
                continue;
            }

            // Flag stragglers (≥10× median)
            for (branch_idx, dur) in &durations {
                let ratio = *dur as f64 / median as f64;
                if ratio >= 10.0 {
                    let msg = format!(
                        "Fork at {} ({} branches): branch #{} took {} ({:.1}x median {})",
                        format_ms(*fork_ts),
                        branch_count,
                        branch_idx,
                        format_ms(*dur),
                        ratio,
                        format_ms(median),
                    );
                    let end_ts = fork_ts + dur;
                    findings.push(
                        Finding::new(Severity::Warning, "branch-imbalance", msg)
                            .with_time(*fork_ts, Some(end_ts)),
                    );
                }
            }
        }

        findings
    }
}

// ── B. gc-storm ─────────────────────────────────────────────────────────────

struct GcStormAcc {
    /// (timestamp_ns, duration_ns)
    safepoints: Vec<(u64, u64)>,
}

impl GcStormAcc {
    fn new() -> Self {
        Self { safepoints: Vec::new() }
    }

    fn record(&mut self, timestamp_ns: u64, duration_ns: u64) {
        self.safepoints.push((timestamp_ns, duration_ns));
    }

    fn finalize(mut self) -> Vec<Finding> {
        let mut findings = Vec::new();

        if self.safepoints.len() < 3 {
            return findings;
        }

        self.safepoints.sort_by_key(|(ts, _)| *ts);

        const STORM_GAP_NS: u64 = 10_000_000; // 10ms
        const MIN_RUN: usize = 3;

        let mut run_start = 0;
        let mut i = 1;

        while i < self.safepoints.len() {
            let gap = self.safepoints[i].0.saturating_sub(self.safepoints[i - 1].0);
            if gap >= STORM_GAP_NS {
                // End of run
                let run_len = i - run_start;
                if run_len >= MIN_RUN {
                    Self::emit_run(&self.safepoints[run_start..i], &mut findings);
                }
                run_start = i;
            }
            i += 1;
        }
        // Check final run
        let run_len = self.safepoints.len() - run_start;
        if run_len >= MIN_RUN {
            Self::emit_run(&self.safepoints[run_start..], &mut findings);
        }

        findings
    }

    fn emit_run(run: &[(u64, u64)], findings: &mut Vec<Finding>) {
        let start_ts = run[0].0;
        let end_ts = run.last().expect("run must be non-empty").0;
        let span_ns = end_ts.saturating_sub(start_ts);
        let count = run.len();
        let avg_gap_ns = if count > 1 { span_ns / (count as u64 - 1) } else { 0 };

        let msg = format!(
            "{} GC safepoints in {} (avg {} apart), sustained allocation pressure",
            count,
            format_ms(span_ns),
            format_ms(avg_gap_ns),
        );
        findings.push(
            Finding::new(Severity::Warning, "gc-storm", msg)
                .with_time(start_ts, Some(end_ts)),
        );
    }
}

// ── C. tier-thrash ──────────────────────────────────────────────────────────

struct TierHistoryEntry {
    timestamp_ns: u64,
    tier: TraceTier,
    #[allow(dead_code)]
    exec_count: u32,
}

struct BailoutEntry {
    #[allow(dead_code)]
    timestamp_ns: u64,
    reason: String,
}

struct TierHistory {
    dispatches: Vec<TierHistoryEntry>,
    bailouts: Vec<BailoutEntry>,
}

struct TierThrashAcc {
    histories: HashMap<u64, TierHistory>,
    /// Maps thread_id → last dispatched expression_hash (for attributing bailouts)
    last_dispatch: HashMap<u32, u64>,
}

impl TierThrashAcc {
    fn new() -> Self {
        Self { histories: HashMap::new(), last_dispatch: HashMap::new() }
    }

    fn record_dispatch(
        &mut self,
        thread_id: u32,
        timestamp_ns: u64,
        expression_hash: u64,
        selected_tier: TraceTier,
        execution_count: u32,
    ) {
        self.last_dispatch.insert(thread_id, expression_hash);
        let history = self.histories.entry(expression_hash).or_insert_with(|| TierHistory {
            dispatches: Vec::new(),
            bailouts: Vec::new(),
        });
        history.dispatches.push(TierHistoryEntry {
            timestamp_ns,
            tier: selected_tier,
            exec_count: execution_count,
        });
    }

    fn record_bailout(&mut self, thread_id: u32, timestamp_ns: u64, reason: String) {
        if let Some(&expr_hash) = self.last_dispatch.get(&thread_id) {
            let history = self.histories.entry(expr_hash).or_insert_with(|| TierHistory {
                dispatches: Vec::new(),
                bailouts: Vec::new(),
            });
            history.bailouts.push(BailoutEntry { timestamp_ns, reason });
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        for (expr_hash, history) in &self.histories {
            if history.bailouts.len() < 2 {
                continue;
            }

            // Build tier sequence string
            let mut tier_seq = Vec::new();
            let mut dispatch_iter = history.dispatches.iter().peekable();
            let mut bailout_idx = 0;

            // Interleave dispatches and bailouts by timestamp
            while dispatch_iter.peek().is_some() || bailout_idx < history.bailouts.len() {
                let take_dispatch = match (dispatch_iter.peek(), history.bailouts.get(bailout_idx)) {
                    (Some(d), Some(b)) => d.timestamp_ns <= b.timestamp_ns,
                    (Some(_), None) => true,
                    (None, Some(_)) => false,
                    (None, None) => break,
                };

                if take_dispatch {
                    let d = dispatch_iter.next().expect("peek confirmed Some");
                    tier_seq.push(format!("{}", d.tier));
                } else {
                    let b = &history.bailouts[bailout_idx];
                    tier_seq.push(format!("bailout:{}", truncate_str(&b.reason, 30)));
                    bailout_idx += 1;
                }
            }

            let first_ts = history.dispatches.first()
                .map(|d| d.timestamp_ns)
                .unwrap_or(0);

            let msg = format!(
                "Expression {:#x} bounced {} times: {}",
                expr_hash,
                history.bailouts.len(),
                tier_seq.join("→"),
            );
            findings.push(
                Finding::new(Severity::Warning, "tier-thrash", msg)
                    .with_time(first_ts, None),
            );
        }

        findings
    }
}

// ── D. workpool-saturation ──────────────────────────────────────────────────

struct WorkpoolSnapshot {
    timestamp_ns: u64,
    queue_depth: u32,
    active_workers: u32,
}

struct WorkpoolSaturationAcc {
    snapshots: Vec<WorkpoolSnapshot>,
    drop_count: u64,
    /// Parked while queue had items
    starvation_count: u64,
}

impl WorkpoolSaturationAcc {
    fn new() -> Self {
        Self { snapshots: Vec::new(), drop_count: 0, starvation_count: 0 }
    }

    fn record_snapshot(&mut self, timestamp_ns: u64, queue_depth: u32, active_workers: u32) {
        self.snapshots.push(WorkpoolSnapshot { timestamp_ns, queue_depth, active_workers });
    }

    fn record_drop(&mut self) {
        self.drop_count += 1;
    }

    fn record_parked(&mut self, queue_depth: u32) {
        if queue_depth > 0 {
            self.starvation_count += 1;
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Detect sustained queue_depth > active_workers
        const MIN_RUN: usize = 5;
        let mut run_start: Option<usize> = None;

        for (i, snap) in self.snapshots.iter().enumerate() {
            let saturated = snap.queue_depth > snap.active_workers;
            match (saturated, run_start) {
                (true, None) => run_start = Some(i),
                (false, Some(start)) => {
                    let run_len = i - start;
                    if run_len >= MIN_RUN {
                        Self::emit_saturation(&self.snapshots[start..i], &mut findings);
                    }
                    run_start = None;
                }
                _ => {}
            }
        }
        // Check trailing run
        if let Some(start) = run_start {
            let run_len = self.snapshots.len() - start;
            if run_len >= MIN_RUN {
                Self::emit_saturation(&self.snapshots[start..], &mut findings);
            }
        }

        // Report drops
        if self.drop_count > 0 {
            let msg = format!(
                "{} compile tasks dropped due to backpressure",
                self.drop_count,
            );
            findings.push(Finding::new(Severity::Warning, "workpool-saturation", msg));
        }

        // Report starvation
        if self.starvation_count > 0 {
            let msg = format!(
                "{} worker-parked events while queue was non-empty (potential starvation)",
                self.starvation_count,
            );
            findings.push(Finding::new(Severity::Warning, "workpool-saturation", msg));
        }

        findings
    }

    fn emit_saturation(run: &[WorkpoolSnapshot], findings: &mut Vec<Finding>) {
        let start_ts = run[0].timestamp_ns;
        let end_ts = run.last().expect("run must be non-empty").timestamp_ns;
        let span_ns = end_ts.saturating_sub(start_ts);
        let count = run.len();

        let msg = format!(
            "Queue saturated for {} (queue_depth > active_workers for {} snapshots)",
            format_ms(span_ns),
            count,
        );
        findings.push(
            Finding::new(Severity::Warning, "workpool-saturation", msg)
                .with_time(start_ts, Some(end_ts)),
        );
    }
}

// ── E. sequential-fan-out ───────────────────────────────────────────────────

struct ForkRange {
    fork_timestamp_ns: u64,
    thread_id: u32,
    branch_count: u32,
    first_branch_seq: Option<u64>,
    last_branch_seq: Option<u64>,
}

struct SequentialFanOutAcc {
    /// Active forks keyed by (thread_id, fork_timestamp)
    active_forks: Vec<ForkRange>,
    /// WorkPool task event seq numbers for overlap detection
    workpool_seqs: Vec<(u32, u64)>, // (thread_id, seq)
}

impl SequentialFanOutAcc {
    fn new() -> Self {
        Self { active_forks: Vec::new(), workpool_seqs: Vec::new() }
    }

    fn record_fork(&mut self, timestamp_ns: u64, thread_id: u32, branch_count: u32) {
        self.active_forks.push(ForkRange {
            fork_timestamp_ns: timestamp_ns,
            thread_id,
            branch_count,
            first_branch_seq: None,
            last_branch_seq: None,
        });
    }

    fn record_branch_start(&mut self, thread_id: u32, seq: u64) {
        // Update the most recent fork on this thread
        for fork in self.active_forks.iter_mut().rev() {
            if fork.thread_id == thread_id {
                if fork.first_branch_seq.is_none() {
                    fork.first_branch_seq = Some(seq);
                }
                fork.last_branch_seq = Some(seq);
                break;
            }
        }
    }

    fn record_branch_end(&mut self, thread_id: u32, seq: u64) {
        for fork in self.active_forks.iter_mut().rev() {
            if fork.thread_id == thread_id {
                fork.last_branch_seq = Some(seq);
                break;
            }
        }
    }

    fn record_workpool_event(&mut self, thread_id: u32, seq: u64) {
        self.workpool_seqs.push((thread_id, seq));
    }

    fn finalize(mut self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Sort workpool events by (thread_id, seq) for efficient overlap detection
        self.workpool_seqs.sort_unstable();

        for fork in &self.active_forks {
            if fork.branch_count < 10 {
                continue;
            }

            let (first_seq, last_seq) = match (fork.first_branch_seq, fork.last_branch_seq) {
                (Some(f), Some(l)) => (f, l),
                _ => continue,
            };

            // Check if any workpool activity falls within the fork's seq range
            // on ANY thread (parallel work could be on a different thread)
            let has_parallel = self.workpool_seqs.iter()
                .any(|(_, s)| *s >= first_seq && *s <= last_seq);

            if !has_parallel {
                let msg = format!(
                    "Fork at {} with {} branches executed sequentially (no work pool activity). Parallelism opportunity.",
                    format_ms(fork.fork_timestamp_ns),
                    fork.branch_count,
                );
                findings.push(
                    Finding::new(Severity::Info, "sequential-fan-out", msg)
                        .with_time(fork.fork_timestamp_ns, None),
                );
            }
        }

        // Deduplicate: keep only the fork with the highest branch count if multiple
        // forks overlap at the same timestamp
        findings.sort_by_key(|f| f.time_range.map(|(ts, _)| ts).unwrap_or(0));
        findings
    }
}

// ── F. gc-pause-outlier ─────────────────────────────────────────────────────

struct GcPauseOutlierAcc {
    /// (timestamp_ns, duration_ns)
    pauses: Vec<(u64, u64)>,
}

impl GcPauseOutlierAcc {
    fn new() -> Self {
        Self { pauses: Vec::new() }
    }

    fn record(&mut self, timestamp_ns: u64, duration_ns: u64) {
        self.pauses.push((timestamp_ns, duration_ns));
    }

    fn finalize(mut self) -> Vec<Finding> {
        let mut findings = Vec::new();

        if self.pauses.len() < 5 {
            return findings;
        }

        // Compute P95
        self.pauses.sort_by_key(|(_, d)| *d);
        let p95_idx = (self.pauses.len() as f64 * 0.95) as usize;
        let p95_idx = p95_idx.min(self.pauses.len() - 1);
        let p95 = self.pauses[p95_idx].1;

        // Find outliers above P95
        let outliers: Vec<(u64, u64)> = self.pauses.iter()
            .filter(|(_, d)| *d > p95)
            .copied()
            .collect();

        if outliers.is_empty() {
            return findings;
        }

        // Sort outliers by duration descending
        let mut sorted_outliers = outliers;
        sorted_outliers.sort_by(|a, b| b.1.cmp(&a.1));

        let severity = if p95 > 1_000_000 { Severity::Warning } else { Severity::Info };

        let worst = sorted_outliers[0];
        let report_count = sorted_outliers.len().min(5);
        let msg = format!(
            "{} GC pauses exceed P95 ({}). Worst: {} at {}",
            sorted_outliers.len(),
            format_ms(p95),
            format_ms(worst.1),
            format_ms(worst.0),
        );
        let mut finding = Finding::new(severity, "gc-pause-outlier", msg);
        if report_count > 1 {
            let first_ts = sorted_outliers.last().expect("non-empty").0;
            let last_ts = sorted_outliers[0].0;
            finding = finding.with_time(first_ts, Some(last_ts));
        } else {
            finding = finding.with_time(worst.0, None);
        }
        findings.push(finding);

        findings
    }
}

// ── G. eval-depth-explosion ─────────────────────────────────────────────────

struct EvalDepthExplosionAcc {
    threshold: u32,
    max_depth: u32,
    violation_count: u64,
    /// First few violation timestamps
    first_violations: Vec<u64>,
}

impl EvalDepthExplosionAcc {
    fn new(threshold: u32) -> Self {
        Self {
            threshold,
            max_depth: 0,
            violation_count: 0,
            first_violations: Vec::new(),
        }
    }

    fn record(&mut self, depth: u32, timestamp_ns: u64) {
        if depth > self.max_depth {
            self.max_depth = depth;
        }
        if depth > self.threshold {
            self.violation_count += 1;
            if self.first_violations.len() < 5 {
                self.first_violations.push(timestamp_ns);
            }
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        if self.violation_count == 0 {
            return findings;
        }

        let first_ts = self.first_violations.first().copied().unwrap_or(0);
        let msg = format!(
            "Max eval depth {} (threshold: {}). {} events exceeded threshold; first at {}",
            self.max_depth,
            self.threshold,
            self.violation_count,
            format_ms(first_ts),
        );
        findings.push(
            Finding::new(Severity::Warning, "eval-depth-explosion", msg)
                .with_time(first_ts, None),
        );

        findings
    }
}

// ── H. compilation-latency ──────────────────────────────────────────────────

struct CompilationRecord {
    timestamp_ns: u64,
    duration_ns: u64,
    kind: &'static str, // "bytecode" or "jit"
}

struct CompilationLatencyAcc {
    /// expression_hash → Vec<CompilationRecord>
    compilations: HashMap<u64, Vec<CompilationRecord>>,
}

impl CompilationLatencyAcc {
    fn new() -> Self {
        Self { compilations: HashMap::new() }
    }

    fn record_bytecode(&mut self, expression_hash: u64, timestamp_ns: u64, duration_ns: u64) {
        self.compilations.entry(expression_hash).or_default().push(CompilationRecord {
            timestamp_ns,
            duration_ns,
            kind: "bytecode",
        });
    }

    fn record_jit(&mut self, expression_hash: u64, timestamp_ns: u64, duration_ns: u64) {
        self.compilations.entry(expression_hash).or_default().push(CompilationRecord {
            timestamp_ns,
            duration_ns,
            kind: "jit",
        });
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        const EXPENSIVE_THRESHOLD_NS: u64 = 5_000_000; // 5ms

        for (expr_hash, records) in &self.compilations {
            let total_duration: u64 = records.iter().map(|r| r.duration_ns).sum();
            let bytecode_count = records.iter().filter(|r| r.kind == "bytecode").count();
            let jit_count = records.iter().filter(|r| r.kind == "jit").count();

            // Flag expressions compiled multiple times
            if records.len() >= 2 {
                let first_ts = records.iter().map(|r| r.timestamp_ns).min().unwrap_or(0);
                let msg = format!(
                    "Expression {:#x} compiled {} times (bytecode\u{00d7}{}, jit\u{00d7}{}), total {} compile time",
                    expr_hash,
                    records.len(),
                    bytecode_count,
                    jit_count,
                    format_ms(total_duration),
                );
                findings.push(
                    Finding::new(Severity::Warning, "compilation-latency", msg)
                        .with_time(first_ts, None),
                );
            }

            // Flag expensive individual compilations
            for record in records {
                if record.duration_ns >= EXPENSIVE_THRESHOLD_NS {
                    let msg = format!(
                        "Expression {:#x}: {} compilation took {}",
                        expr_hash,
                        record.kind,
                        format_ms(record.duration_ns),
                    );
                    // Only Info if it was a single compilation, Warning if repeated
                    let severity = if records.len() >= 2 { Severity::Warning } else { Severity::Info };
                    findings.push(
                        Finding::new(severity, "compilation-latency", msg)
                            .with_time(record.timestamp_ns, None),
                    );
                }
            }
        }

        findings
    }
}

// ── LintPass — orchestrates all 8 accumulators ──────────────────────────────

struct LintPass {
    branch_imbalance: Option<BranchImbalanceAcc>,
    gc_storm: Option<GcStormAcc>,
    tier_thrash: Option<TierThrashAcc>,
    workpool_saturation: Option<WorkpoolSaturationAcc>,
    sequential_fan_out: Option<SequentialFanOutAcc>,
    gc_pause_outlier: Option<GcPauseOutlierAcc>,
    eval_depth_explosion: Option<EvalDepthExplosionAcc>,
    compilation_latency: Option<CompilationLatencyAcc>,
}

impl LintPass {
    fn new(config: &LintConfig) -> Self {
        Self {
            branch_imbalance: config.is_enabled("branch-imbalance")
                .then(BranchImbalanceAcc::new),
            gc_storm: config.is_enabled("gc-storm")
                .then(GcStormAcc::new),
            tier_thrash: config.is_enabled("tier-thrash")
                .then(TierThrashAcc::new),
            workpool_saturation: config.is_enabled("workpool-saturation")
                .then(WorkpoolSaturationAcc::new),
            sequential_fan_out: config.is_enabled("sequential-fan-out")
                .then(SequentialFanOutAcc::new),
            gc_pause_outlier: config.is_enabled("gc-pause-outlier")
                .then(GcPauseOutlierAcc::new),
            eval_depth_explosion: config.is_enabled("eval-depth-explosion")
                .then(|| EvalDepthExplosionAcc::new(config.depth_threshold)),
            compilation_latency: config.is_enabled("compilation-latency")
                .then(CompilationLatencyAcc::new),
        }
    }

    fn process(&mut self, event: &trace_format::TraceEvent) {
        let ts = event.timestamp_ns;
        let tid = event.thread_id;
        let seq = event.seq;
        let dur = event.duration_ns.unwrap_or(0);

        // G. eval-depth-explosion — checks every event's depth
        if let Some(acc) = &mut self.eval_depth_explosion {
            acc.record(event.depth, ts);
        }

        match &event.kind {
            // ── Nondeterminism events ──
            TraceEventKind::NondeterministicFork { branch_count } => {
                if let Some(acc) = &mut self.branch_imbalance {
                    acc.record_fork(ts, tid, *branch_count);
                }
                if let Some(acc) = &mut self.sequential_fan_out {
                    acc.record_fork(ts, tid, *branch_count);
                }
            }

            TraceEventKind::BranchStart { .. } => {
                if let Some(acc) = &mut self.sequential_fan_out {
                    acc.record_branch_start(tid, seq);
                }
            }

            TraceEventKind::BranchEnd { branch_index, .. } => {
                if let Some(acc) = &mut self.branch_imbalance {
                    acc.record_branch_end(ts, tid, event.duration_ns, *branch_index);
                }
                if let Some(acc) = &mut self.sequential_fan_out {
                    acc.record_branch_end(tid, seq);
                }
            }

            // ── GC events ──
            TraceEventKind::GcSafepoint { .. } => {
                if let Some(acc) = &mut self.gc_storm {
                    acc.record(ts, dur);
                }
                if let Some(acc) = &mut self.gc_pause_outlier {
                    if dur > 0 {
                        acc.record(ts, dur);
                    }
                }
            }

            // ── Tier transition events ──
            TraceEventKind::TierDispatch { expression_hash, selected_tier, execution_count } => {
                if let Some(acc) = &mut self.tier_thrash {
                    acc.record_dispatch(tid, ts, *expression_hash, *selected_tier, *execution_count);
                }
            }

            TraceEventKind::JitBailout { reason, .. } => {
                if let Some(acc) = &mut self.tier_thrash {
                    acc.record_bailout(tid, ts, reason.clone());
                }
            }

            TraceEventKind::BytecodeHalt { reason, .. } => {
                if let Some(acc) = &mut self.tier_thrash {
                    acc.record_bailout(tid, ts, reason.clone());
                }
            }

            // ── Compilation events ──
            TraceEventKind::BytecodeCompilation { expression_hash, .. } => {
                if let Some(acc) = &mut self.compilation_latency {
                    acc.record_bytecode(*expression_hash, ts, dur);
                }
            }

            TraceEventKind::JitCompilation { expression_hash, .. } => {
                if let Some(acc) = &mut self.compilation_latency {
                    acc.record_jit(*expression_hash, ts, dur);
                }
            }

            // ── WorkPool events ──
            TraceEventKind::WorkPoolTaskEnqueued { queue_depth, active_workers, .. } => {
                if let Some(acc) = &mut self.workpool_saturation {
                    acc.record_snapshot(ts, *queue_depth, *active_workers);
                }
                if let Some(acc) = &mut self.sequential_fan_out {
                    acc.record_workpool_event(tid, seq);
                }
            }

            TraceEventKind::WorkPoolTaskDropped { .. } => {
                if let Some(acc) = &mut self.workpool_saturation {
                    acc.record_drop();
                }
            }

            TraceEventKind::WorkPoolTaskCompleted { queue_depth, active_workers, .. } => {
                if let Some(acc) = &mut self.workpool_saturation {
                    acc.record_snapshot(ts, *queue_depth, *active_workers);
                }
                if let Some(acc) = &mut self.sequential_fan_out {
                    acc.record_workpool_event(tid, seq);
                }
            }

            TraceEventKind::WorkPoolWorkerParked { queue_depth, .. } => {
                if let Some(acc) = &mut self.workpool_saturation {
                    acc.record_parked(*queue_depth);
                }
            }

            // All other event kinds — no lint accumulator cares
            _ => {}
        }
    }

    fn finalize(self, config: &LintConfig) -> Vec<Finding> {
        let mut findings = Vec::new();

        if let Some(acc) = self.branch_imbalance {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.gc_storm {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.tier_thrash {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.workpool_saturation {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.sequential_fan_out {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.gc_pause_outlier {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.eval_depth_explosion {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.compilation_latency {
            findings.extend(acc.finalize());
        }

        // Filter by severity
        findings.retain(|f| f.severity >= config.min_severity);

        // Sort by timestamp (findings without timestamps last)
        findings.sort_by(|a, b| {
            let a_ts = a.time_range.map(|(ts, _)| ts).unwrap_or(u64::MAX);
            let b_ts = b.time_range.map(|(ts, _)| ts).unwrap_or(u64::MAX);
            a_ts.cmp(&b_ts)
        });

        findings
    }
}

// ── Output ──────────────────────────────────────────────────────────────────

fn print_findings(reader: &TraceReader, config: &LintConfig, findings: &[Finding]) {
    let enabled_lints: Vec<&str> = LINT_IDS.iter()
        .filter(|id| config.is_enabled(id))
        .copied()
        .collect();

    println!("=== Lint Results ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Format: v{}", reader.format_version);
    println!("Lints run: {}", enabled_lints.join(", "));
    println!();

    if findings.is_empty() {
        println!("No findings.");
    } else {
        for finding in findings {
            print!("[{}] {}: {}", finding.severity, finding.lint_id, finding.message);
            println!();

            if let Some((start, end)) = finding.time_range {
                if let Some(end_ns) = end {
                    println!("  {} - {}", format_ms(start), format_ms(end_ns));
                } else {
                    println!("  {}", format_ms(start));
                }
            }

            println!();
        }
    }

    // Summary
    let warning_count = findings.iter().filter(|f| f.severity == Severity::Warning).count();
    let info_count = findings.iter().filter(|f| f.severity == Severity::Info).count();

    println!("--- Summary ---");

    let mut parts = Vec::new();
    if warning_count > 0 {
        parts.push(format!("{} warning{}", warning_count, if warning_count == 1 { "" } else { "s" }));
    }
    if info_count > 0 {
        parts.push(format!("{} info", info_count));
    }

    let total = findings.len();
    if total > 0 {
        println!("  {} ({} finding{})", parts.join(", "), total, if total == 1 { "" } else { "s" });
    } else {
        println!("  0 findings");
    }

    // Report clean lints
    let lint_ids_with_findings: Vec<&str> = findings.iter()
        .map(|f| f.lint_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let clean_lints: Vec<&str> = enabled_lints.iter()
        .filter(|id| !lint_ids_with_findings.contains(*id))
        .copied()
        .collect();

    if !clean_lints.is_empty() {
        println!("  Lints clean: {}", clean_lints.join(", "));
    }
}

// ── Entry point ─────────────────────────────────────────────────────────────

pub fn run(
    file: &str,
    severity: &str,
    lint_ids: Option<&str>,
    depth_threshold: u32,
) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    if reader.format_version < 2 {
        return Err("Lint analysis requires trace format v2 (duration_ns/span_id fields). \
                    Re-record with a current mettatron build.".to_string());
    }

    let config = LintConfig {
        min_severity: severity.parse()?,
        enabled_lints: lint_ids.map(|s| {
            s.split(',').map(|id| id.trim().to_string()).collect()
        }),
        depth_threshold,
    };

    // Validate lint IDs
    if let Some(ids) = &config.enabled_lints {
        for id in ids {
            if !LINT_IDS.contains(&id.as_str()) {
                return Err(format!(
                    "Unknown lint: {id:?}. Available: {}",
                    LINT_IDS.join(", "),
                ));
            }
        }
    }

    let mut pass = LintPass::new(&config);

    // Single-pass sweep
    for event in reader.events() {
        pass.process(&event);
    }

    let findings = pass.finalize(&config);
    print_findings(&reader, &config, &findings);

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find a valid UTF-8 boundary
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}
