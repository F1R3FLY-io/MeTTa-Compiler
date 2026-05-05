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
    "workpool-oscillation",
    "workpool-floor-pinned",
    "workpool-emergency-storm",
    "workpool-pressure-dominance",
    "workpool-convergence",
    // Phase A6 lints
    "repeated-eval",
    "empty-branch-ratio",
    "rule-match-explosion",
    "type-inference-miss",
    "fork-depth-explosion",
    "speculative-waste",
    "gc-allocation-hotspot",
    "tier-promotion-opportunity",
];

// ── Configuration ───────────────────────────────────────────────────────────

pub struct LintConfig {
    pub min_severity: Severity,
    pub enabled_lints: Option<Vec<String>>,
    pub depth_threshold: u32,
    /// Threshold for repeated-eval lint: flag after N identical evaluations (default: 3).
    pub repeated_eval_threshold: u32,
    /// Threshold for rule-match-explosion: flag when match_count exceeds N (default: 10).
    pub rule_match_threshold: u32,
    /// Threshold for fork-depth-explosion: flag when fork nesting exceeds N (default: 3).
    pub fork_depth_threshold: u32,
    /// Threshold for tier-promotion-opportunity: execution count before flagging (default: 100).
    pub tier_promotion_threshold: u32,
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
    /// **H7 Stage 2 (2026-05-05)**: span_id from the wrapping `TraceEvent`.
    /// `None` means the emitter didn't pair this with a BranchStart (e.g.,
    /// malformed shim-path emission, now suppressed by H7 Stage 1).
    span_id: Option<u64>,
}

struct BranchStartRecord {
    timestamp_ns: u64,
    thread_id: u32,
    branch_index: u32,
    /// span_id correlation key for this branch.
    span_id: u64,
}

struct BranchImbalanceAcc {
    forks: Vec<ForkRecord>,
    branch_ends: Vec<BranchEndRecord>,
    /// H7 Stage 2: BranchStart records for span_id-based correlation.
    /// When span_ids match between BranchStart and BranchEnd, lint groups
    /// by span_id (precise). When span_id is missing, falls back to the
    /// existing timestamp-nearest-preceding-fork heuristic.
    branch_starts: Vec<BranchStartRecord>,
}

impl BranchImbalanceAcc {
    fn new() -> Self {
        Self {
            forks: Vec::new(),
            branch_ends: Vec::new(),
            branch_starts: Vec::new(),
        }
    }

    fn record_fork(&mut self, timestamp_ns: u64, thread_id: u32, branch_count: u32) {
        self.forks.push(ForkRecord { timestamp_ns, thread_id, branch_count });
    }

    fn record_branch_start(
        &mut self,
        timestamp_ns: u64,
        thread_id: u32,
        branch_index: u32,
        span_id: u64,
    ) {
        if span_id != 0 {
            self.branch_starts
                .push(BranchStartRecord { timestamp_ns, thread_id, branch_index, span_id });
        }
    }

    fn record_branch_end(
        &mut self,
        timestamp_ns: u64,
        thread_id: u32,
        duration_ns: Option<u64>,
        branch_index: u32,
        span_id: Option<u64>,
    ) {
        self.branch_ends.push(BranchEndRecord {
            timestamp_ns,
            thread_id,
            duration_ns,
            branch_index,
            span_id,
        });
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // H7 Stage 2 (2026-05-05): build span_id → BranchStart map for precise
        // pairing. BranchEnds with a span_id that matches a BranchStart are
        // grouped by their fork-anchor (the BranchStart's preceding fork);
        // BranchEnds without span_id (legacy/malformed) fall back to the
        // timestamp-partition heuristic.
        let starts_by_span: HashMap<u64, &BranchStartRecord> = self
            .branch_starts
            .iter()
            .map(|bs| (bs.span_id, bs))
            .collect();

        // Build per-thread fork lists sorted by timestamp for binary search
        let mut forks_by_thread: HashMap<u32, Vec<&ForkRecord>> = HashMap::new();
        for fork in &self.forks {
            forks_by_thread.entry(fork.thread_id).or_default().push(fork);
        }
        for forks in forks_by_thread.values_mut() {
            forks.sort_by_key(|f| f.timestamp_ns);
        }

        // Group branch ends to their nearest preceding fork on the same thread.
        // Prefer span_id correlation: BranchEnd with span_id S → match BranchStart
        // with same span_id, then look up that BranchStart's anchor fork.
        let mut fork_groups: HashMap<(u32, u64), (u32, Vec<&BranchEndRecord>)> = HashMap::new();

        for be in &self.branch_ends {
            // H7 Stage 2: prefer span_id correlation
            let anchor_ts = if let Some(span) = be.span_id {
                if let Some(bs) = starts_by_span.get(&span) {
                    if let Some(forks) = forks_by_thread.get(&bs.thread_id) {
                        let idx = forks.partition_point(|f| f.timestamp_ns <= bs.timestamp_ns);
                        if idx > 0 { Some(forks[idx - 1]) } else { None }
                    } else { None }
                } else {
                    // span_id present but no matching BranchStart — drop this
                    // event (it's an orphan, likely from an emission bug).
                    continue;
                }
            } else if let Some(forks) = forks_by_thread.get(&be.thread_id) {
                // Fallback: timestamp-partition heuristic
                let idx = forks.partition_point(|f| f.timestamp_ns <= be.timestamp_ns);
                if idx > 0 { Some(forks[idx - 1]) } else { None }
            } else {
                None
            };
            if let Some(fork) = anchor_ts {
                let key = (fork.thread_id, fork.timestamp_ns);
                let entry = fork_groups.entry(key).or_insert_with(|| (fork.branch_count, Vec::new()));
                entry.1.push(be);
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

// ── I. workpool-oscillation ──────────────────────────────────────────────────

struct WorkpoolOscillationAcc {
    /// (timestamp_ns, action) pairs for scale events
    actions: Vec<(u64, String)>,
}

impl WorkpoolOscillationAcc {
    fn new() -> Self {
        Self { actions: Vec::new() }
    }

    fn record(&mut self, timestamp_ns: u64, action: &str) {
        self.actions.push((timestamp_ns, action.to_string()));
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Look for 3+ alternating park/unpark within 5 consecutive ticks
        const WINDOW: usize = 5;
        const MIN_ALTERNATIONS: usize = 3;

        if self.actions.len() < WINDOW {
            return findings;
        }

        for start in 0..=self.actions.len() - WINDOW {
            let window = &self.actions[start..start + WINDOW];
            let mut alternations = 0;
            for pair in window.windows(2) {
                let a = &pair[0].1;
                let b = &pair[1].1;
                if (a == "park" && b == "unpark") || (a == "unpark" && b == "park") {
                    alternations += 1;
                }
            }
            if alternations >= MIN_ALTERNATIONS {
                let start_ts = window[0].0;
                let end_ts = window.last().expect("window must be non-empty").0;
                let actions: Vec<&str> = window.iter().map(|(_, a)| a.as_str()).collect();
                let msg = format!(
                    "Hill climber oscillating: {} alternating park/unpark in {} ticks [{}]",
                    alternations,
                    WINDOW,
                    actions.join(" → "),
                );
                findings.push(
                    Finding::new(Severity::Warning, "workpool-oscillation", msg)
                        .with_time(start_ts, Some(end_ts)),
                );
            }
        }

        findings
    }
}

// ── J. workpool-floor-pinned ────────────────────────────────────────────────

struct WorkpoolFloorPinnedAcc {
    /// (timestamp_ns, active_workers_after, min_workers, dominant_term_name)
    ticks: Vec<(u64, u32, u32, String)>,
}

impl WorkpoolFloorPinnedAcc {
    fn new() -> Self {
        Self { ticks: Vec::new() }
    }

    fn record(
        &mut self,
        timestamp_ns: u64,
        active_workers_after: u32,
        min_workers: u32,
        term_throughput: f64,
        term_queue_depth: f64,
        term_slab_pressure: f64,
        term_rss_pressure: f64,
    ) {
        let dominant = Self::dominant_term(
            term_throughput, term_queue_depth, term_slab_pressure, term_rss_pressure,
        );
        self.ticks.push((timestamp_ns, active_workers_after, min_workers, dominant));
    }

    fn dominant_term(tp: f64, qd: f64, slab: f64, rss: f64) -> String {
        // The objective is sum of all terms; throughput is negative (good),
        // others are positive (bad). The dominant positive term drives parking.
        let terms = [
            ("throughput", tp.abs()),
            ("queue_depth", qd),
            ("slab_pressure", slab),
            ("rss_pressure", rss),
        ];
        terms.iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(name, _)| name.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        const MIN_CONSECUTIVE: usize = 10; // 10 ticks × 200ms = 2 seconds

        let mut run_start: Option<usize> = None;

        for (i, (_, active, min, _)) in self.ticks.iter().enumerate() {
            let at_floor = *active == *min;
            match (at_floor, run_start) {
                (true, None) => run_start = Some(i),
                (false, Some(start)) => {
                    let run_len = i - start;
                    if run_len >= MIN_CONSECUTIVE {
                        Self::emit_floor_pinned(&self.ticks[start..i], &mut findings);
                    }
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start {
            let run_len = self.ticks.len() - start;
            if run_len >= MIN_CONSECUTIVE {
                Self::emit_floor_pinned(&self.ticks[start..], &mut findings);
            }
        }

        findings
    }

    fn emit_floor_pinned(run: &[(u64, u32, u32, String)], findings: &mut Vec<Finding>) {
        let start_ts = run[0].0;
        let end_ts = run.last().expect("run must be non-empty").0;
        let duration_ns = end_ts.saturating_sub(start_ts);
        let min_workers = run[0].2;

        // Count dominant terms
        let mut term_counts: HashMap<String, usize> = HashMap::new();
        for (_, _, _, dominant) in run {
            *term_counts.entry(dominant.clone()).or_insert(0) += 1;
        }
        let top_term = term_counts.iter()
            .max_by_key(|(_, count)| *count)
            .map(|(name, count)| format!("{} ({}×)", name, count))
            .unwrap_or_else(|| "unknown".to_string());

        let msg = format!(
            "Workers pinned at floor ({}) for {} ({} ticks), dominant term: {}",
            min_workers,
            format_ms(duration_ns),
            run.len(),
            top_term,
        );
        findings.push(
            Finding::new(Severity::Warning, "workpool-floor-pinned", msg)
                .with_time(start_ts, Some(end_ts)),
        );
    }
}

// ── K. workpool-emergency-storm ─────────────────────────────────────────────

struct WorkpoolEmergencyStormAcc {
    /// (timestamp_ns, bp_level, slab_pressure, rss_pressure) for emergency ticks
    emergency_ticks: Vec<(u64, u32, f64, f64)>,
}

impl WorkpoolEmergencyStormAcc {
    fn new() -> Self {
        Self { emergency_ticks: Vec::new() }
    }

    fn record(&mut self, timestamp_ns: u64, emergency: bool, bp_level: u32, slab_pressure: f64, rss_pressure: f64) {
        if emergency {
            self.emergency_ticks.push((timestamp_ns, bp_level, slab_pressure, rss_pressure));
        } else {
            // Non-emergency breaks the streak — add a sentinel
            self.emergency_ticks.push((timestamp_ns, u32::MAX, 0.0, 0.0));
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        const MIN_CONSECUTIVE: usize = 5; // 5 ticks × 200ms = 1 second

        let mut run_start: Option<usize> = None;

        for (i, &(_, bp_level, _, _)) in self.emergency_ticks.iter().enumerate() {
            let is_emergency = bp_level != u32::MAX;
            match (is_emergency, run_start) {
                (true, None) => run_start = Some(i),
                (false, Some(start)) => {
                    let run_len = i - start;
                    if run_len >= MIN_CONSECUTIVE {
                        Self::emit_storm(&self.emergency_ticks[start..i], &mut findings);
                    }
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start {
            // Filter out sentinel entries
            let actual: Vec<_> = self.emergency_ticks[start..]
                .iter()
                .filter(|&&(_, bp, _, _)| bp != u32::MAX)
                .collect();
            if actual.len() >= MIN_CONSECUTIVE {
                Self::emit_storm(&self.emergency_ticks[start..], &mut findings);
            }
        }

        findings
    }

    fn emit_storm(run: &[(u64, u32, f64, f64)], findings: &mut Vec<Finding>) {
        let actual: Vec<_> = run.iter().filter(|&&(_, bp, _, _)| bp != u32::MAX).collect();
        if actual.is_empty() {
            return;
        }
        let start_ts = actual[0].0;
        let end_ts = actual.last().expect("actual must be non-empty").0;
        let duration_ns = end_ts.saturating_sub(start_ts);
        let max_bp: u32 = actual.iter().map(|&&(_, bp, _, _)| bp).max().unwrap_or(0);
        let max_slab: f64 = actual.iter().map(|&&(_, _, s, _)| s).fold(0.0_f64, f64::max);
        let max_rss: f64 = actual.iter().map(|&&(_, _, _, r)| r).fold(0.0_f64, f64::max);

        let msg = format!(
            "Emergency override storm: {} consecutive ticks over {}, max bp_level={}, slab={:.3}, rss={:.3}",
            actual.len(),
            format_ms(duration_ns),
            max_bp,
            max_slab,
            max_rss,
        );
        findings.push(
            Finding::new(Severity::Warning, "workpool-emergency-storm", msg)
                .with_time(start_ts, Some(end_ts)),
        );
    }
}

// ── L. workpool-pressure-dominance ──────────────────────────────────────────

struct WorkpoolPressureDominanceAcc {
    /// (timestamp_ns, term_breakdown) for parking events where memory pressure dominates
    violations: Vec<(u64, f64, f64, f64, f64, u32)>,
}

impl WorkpoolPressureDominanceAcc {
    fn new() -> Self {
        Self { violations: Vec::new() }
    }

    fn record(
        &mut self,
        timestamp_ns: u64,
        action: &str,
        queue_depth: u32,
        term_throughput: f64,
        term_queue_depth: f64,
        term_slab_pressure: f64,
        term_rss_pressure: f64,
    ) {
        // Only flag park events with non-empty queue
        if action != "park" {
            return;
        }
        if queue_depth == 0 {
            return;
        }
        // Check if memory terms dominate throughput + queue terms
        let memory_sum = term_slab_pressure + term_rss_pressure;
        let work_sum = term_throughput.abs() + term_queue_depth;
        if memory_sum > work_sum {
            self.violations.push((
                timestamp_ns,
                term_throughput,
                term_queue_depth,
                term_slab_pressure,
                term_rss_pressure,
                queue_depth,
            ));
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        for &(ts, tp, qd, slab, rss, queue) in &self.violations {
            let msg = format!(
                "Memory pressure starving queue (depth={}): slab={:.4}+rss={:.4} = {:.4} > |tp|={:.4}+qd={:.4} = {:.4}",
                queue, slab, rss, slab + rss, tp.abs(), qd, tp.abs() + qd,
            );
            findings.push(
                Finding::new(Severity::Warning, "workpool-pressure-dominance", msg)
                    .with_time(ts, None),
            );
        }

        findings
    }
}

// ── M. workpool-convergence ─────────────────────────────────────────────────

struct WorkpoolConvergenceAcc {
    /// (timestamp_ns, action, objective)
    ticks: Vec<(u64, String, f64)>,
}

impl WorkpoolConvergenceAcc {
    fn new() -> Self {
        Self { ticks: Vec::new() }
    }

    fn record(&mut self, timestamp_ns: u64, action: &str, objective: f64) {
        self.ticks.push((timestamp_ns, action.to_string(), objective));
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Detect if the hill climber ever converges: sustained Hold with stable objective
        // "Converged" = 10+ consecutive Hold actions with objective variance < 1%
        const CONVERGENCE_WINDOW: usize = 10;

        let mut max_hold_run = 0usize;
        let mut current_hold_run = 0usize;
        let mut ever_converged = false;

        for (_, action, _) in &self.ticks {
            if action == "hold" {
                current_hold_run += 1;
                if current_hold_run > max_hold_run {
                    max_hold_run = current_hold_run;
                }
            } else {
                if current_hold_run >= CONVERGENCE_WINDOW {
                    ever_converged = true;
                }
                current_hold_run = 0;
            }
        }
        if current_hold_run >= CONVERGENCE_WINDOW {
            ever_converged = true;
        }

        if !ever_converged && self.ticks.len() >= CONVERGENCE_WINDOW {
            let msg = format!(
                "Hill climber never converged during trace ({} ticks, max consecutive hold: {})",
                self.ticks.len(),
                max_hold_run,
            );
            findings.push(Finding::new(Severity::Info, "workpool-convergence", msg));
        }

        findings
    }
}

// ── N. repeated-eval ──────────────────────────────────────────────────────

struct RepeatedEvalAcc {
    /// input_hash → (count, head_symbol, first_timestamp)
    seen: HashMap<u64, (u32, String, u64)>,
    threshold: u32,
}

impl RepeatedEvalAcc {
    fn new(threshold: u32) -> Self {
        Self { seen: HashMap::new(), threshold }
    }

    fn record(&mut self, input_hash: u64, head_symbol: &str, timestamp_ns: u64) {
        self.seen.entry(input_hash)
            .and_modify(|(count, _, _)| *count += 1)
            .or_insert((1, head_symbol.to_string(), timestamp_ns));
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        let mut entries: Vec<_> = self.seen.into_iter()
            .filter(|(_, (count, _, _))| *count > self.threshold)
            .collect();
        entries.sort_by(|a, b| b.1.0.cmp(&a.1.0));

        for (hash, (count, head, first_ts)) in entries.into_iter().take(20) {
            let msg = format!(
                "Expression `{}` (hash {:#x}) evaluated {} times (threshold: {})",
                head, hash, count, self.threshold,
            );
            findings.push(
                Finding::new(Severity::Info, "repeated-eval", msg)
                    .with_time(first_ts, None),
            );
        }
        findings
    }
}

// ── O. empty-branch-ratio ────────────────────────────────────────────────

struct EmptyBranchRatioAcc {
    /// fork_hash → (total_branches, empty_branches, first_timestamp, head_symbol)
    forks: HashMap<u64, (u32, u32, u64, String)>,
    /// Current fork context per thread: (fork_hash, head)
    current_fork: HashMap<u32, (u64, String)>,
}

impl EmptyBranchRatioAcc {
    fn new() -> Self {
        Self { forks: HashMap::new(), current_fork: HashMap::new() }
    }

    fn record_fork(&mut self, thread_id: u32, timestamp_ns: u64, branch_count: u32, input_hash: u64, head: &str) {
        self.current_fork.insert(thread_id, (input_hash, head.to_string()));
        self.forks.entry(input_hash)
            .or_insert((0, 0, timestamp_ns, head.to_string()))
            .0 += branch_count;
    }

    fn record_branch_end(&mut self, thread_id: u32, result_count: u32) {
        if let Some((fork_hash, _)) = self.current_fork.get(&thread_id) {
            if let Some(entry) = self.forks.get_mut(fork_hash) {
                if result_count == 0 {
                    entry.1 += 1;
                }
            }
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (hash, (total, empty, ts, head)) in &self.forks {
            if *total < 4 { continue; } // need enough branches to be meaningful
            let ratio = *empty as f64 / *total as f64;
            if ratio > 0.5 {
                let msg = format!(
                    "Fork point `{}` (hash {:#x}): {}/{} branches empty ({:.0}% waste)",
                    head, hash, empty, total, ratio * 100.0,
                );
                findings.push(
                    Finding::new(Severity::Warning, "empty-branch-ratio", msg)
                        .with_time(*ts, None),
                );
            }
        }
        findings
    }
}

// ── P. rule-match-explosion ──────────────────────────────────────────────

struct RuleMatchExplosionAcc {
    threshold: u32,
    /// (timestamp, head_symbol, match_count)
    explosions: Vec<(u64, String, u32)>,
}

impl RuleMatchExplosionAcc {
    fn new(threshold: u32) -> Self {
        Self { threshold, explosions: Vec::new() }
    }

    fn record(&mut self, timestamp_ns: u64, match_count: u32, head: &str) {
        if match_count > self.threshold {
            self.explosions.push((timestamp_ns, head.to_string(), match_count));
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        // Group by head symbol
        let mut by_head: HashMap<String, (u32, u32, u64)> = HashMap::new(); // (count, max_matches, first_ts)
        for (ts, head, match_count) in &self.explosions {
            let entry = by_head.entry(head.clone()).or_insert((0, 0, *ts));
            entry.0 += 1;
            if *match_count > entry.1 {
                entry.1 = *match_count;
            }
        }

        let mut entries: Vec<_> = by_head.into_iter().collect();
        entries.sort_by(|a, b| b.1.1.cmp(&a.1.1));

        for (head, (count, max_matches, first_ts)) in entries.into_iter().take(20) {
            let msg = format!(
                "Rule `{}`: {} occurrences with match_count > {} (max: {})",
                head, count, self.threshold, max_matches,
            );
            findings.push(
                Finding::new(Severity::Warning, "rule-match-explosion", msg)
                    .with_time(first_ts, None),
            );
        }
        findings
    }
}

// ── Q. type-inference-miss ───────────────────────────────────────────────

struct TypeInferenceMissAcc {
    /// (timestamp, expression_display, source)
    misses: Vec<(u64, String, String)>,
}

impl TypeInferenceMissAcc {
    fn new() -> Self {
        Self { misses: Vec::new() }
    }

    fn record(&mut self, timestamp_ns: u64, expression: &str, source: &str) {
        if source.contains("fallback-undefined") {
            self.misses.push((timestamp_ns, expression.to_string(), source.to_string()));
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        let count = self.misses.len();
        if count == 0 {
            return findings;
        }

        // Group by expression head
        let mut by_expr: HashMap<String, u32> = HashMap::new();
        for (_, expr, _) in &self.misses {
            *by_expr.entry(expr.clone()).or_default() += 1;
        }

        let msg = format!(
            "{} type inference fallbacks to %Undefined% ({} distinct expressions)",
            count, by_expr.len(),
        );
        findings.push(Finding::new(Severity::Info, "type-inference-miss", msg));

        // Top 5 most common
        let mut sorted: Vec<_> = by_expr.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (expr, count) in sorted.into_iter().take(5) {
            let expr_display = if expr.len() > 60 {
                format!("{}...", &expr[..57])
            } else {
                expr
            };
            let msg = format!("  `{}`: {} fallbacks", expr_display, count);
            findings.push(Finding::new(Severity::Info, "type-inference-miss", msg));
        }

        findings
    }
}

// ── R. fork-depth-explosion ──────────────────────────────────────────────

struct ForkDepthExplosionAcc {
    threshold: u32,
    /// Per-thread fork nesting depth.
    thread_depth: HashMap<u32, u32>,
    /// (timestamp, thread_id, depth, head) for deep forks.
    violations: Vec<(u64, u32, u32, String)>,
}

impl ForkDepthExplosionAcc {
    fn new(threshold: u32) -> Self {
        Self {
            threshold,
            thread_depth: HashMap::new(),
            violations: Vec::new(),
        }
    }

    fn record_fork(&mut self, timestamp_ns: u64, thread_id: u32, head: &str) {
        let depth = self.thread_depth.entry(thread_id).or_insert(0);
        *depth += 1;
        if *depth > self.threshold {
            self.violations.push((timestamp_ns, thread_id, *depth, head.to_string()));
        }
    }

    fn record_branch_end(&mut self, thread_id: u32) {
        if let Some(depth) = self.thread_depth.get_mut(&thread_id) {
            *depth = depth.saturating_sub(1);
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        if self.violations.is_empty() {
            return findings;
        }

        let max_depth = self.violations.iter().map(|(_, _, d, _)| *d).max().unwrap_or(0);
        let msg = format!(
            "{} fork nesting violations (max depth: {}, threshold: {})",
            self.violations.len(), max_depth, self.threshold,
        );
        findings.push(
            Finding::new(Severity::Warning, "fork-depth-explosion", msg)
                .with_time(self.violations[0].0, None),
        );

        findings
    }
}

// ── S. speculative-waste ─────────────────────────────────────────────────

struct SpeculativeWasteAcc {
    /// Total time in empty branches.
    empty_branch_time_ns: u64,
    /// Total wall time (max timestamp).
    wall_time_ns: u64,
    empty_count: u64,
    total_count: u64,
}

impl SpeculativeWasteAcc {
    fn new() -> Self {
        Self {
            empty_branch_time_ns: 0,
            wall_time_ns: 0,
            empty_count: 0,
            total_count: 0,
        }
    }

    fn record_branch_end(&mut self, result_count: u32, duration_ns: u64) {
        self.total_count += 1;
        if result_count == 0 {
            self.empty_branch_time_ns += duration_ns;
            self.empty_count += 1;
        }
    }

    fn update_wall_time(&mut self, end_ns: u64) {
        if end_ns > self.wall_time_ns {
            self.wall_time_ns = end_ns;
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        if self.wall_time_ns == 0 || self.total_count == 0 {
            return findings;
        }
        let waste_pct = self.empty_branch_time_ns as f64 / self.wall_time_ns as f64;
        if waste_pct > 0.25 {
            let msg = format!(
                "Speculative waste: {}/{} branches empty ({:.1}%), consuming {} ({:.1}% of wall time)",
                self.empty_count,
                self.total_count,
                self.empty_count as f64 / self.total_count as f64 * 100.0,
                format_ms(self.empty_branch_time_ns),
                waste_pct * 100.0,
            );
            findings.push(Finding::new(Severity::Warning, "speculative-waste", msg));
        }
        findings
    }
}

// ── T. gc-allocation-hotspot ─────────────────────────────────────────────

struct GcAllocationHotspotAcc {
    /// depth → (count, total_allocation_delta_bytes)
    by_depth: HashMap<u32, (u64, u64)>,
}

impl GcAllocationHotspotAcc {
    fn new() -> Self {
        Self { by_depth: HashMap::new() }
    }

    fn record(&mut self, depth: u32, allocation_delta_bytes: u64) {
        let entry = self.by_depth.entry(depth).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += allocation_delta_bytes;
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();
        if self.by_depth.is_empty() {
            return findings;
        }

        let total_allocs: u64 = self.by_depth.values().map(|(c, _)| c).sum();
        let total_bytes: u64 = self.by_depth.values().map(|(_, b)| b).sum();

        // Find depth with highest allocation
        let mut sorted: Vec<_> = self.by_depth.into_iter().collect();
        sorted.sort_by(|a, b| b.1.1.cmp(&a.1.1));

        if let Some(&(depth, (count, bytes))) = sorted.first() {
            let pct = if total_bytes > 0 { bytes as f64 / total_bytes as f64 * 100.0 } else { 0.0 };
            if pct > 50.0 {
                let msg = format!(
                    "GC hotspot at depth {}: {} safepoints, {} bytes ({:.0}% of total {})",
                    depth, count, bytes, pct, total_allocs,
                );
                findings.push(Finding::new(Severity::Info, "gc-allocation-hotspot", msg));
            }
        }

        findings
    }
}

// ── U. tier-promotion-opportunity ────────────────────────────────────────

struct TierPromotionOpportunityAcc {
    threshold: u32,
    /// expression_hash → (max_execution_count, max_tier, head_symbol, first_ts)
    dispatches: HashMap<u64, (u32, TraceTier, String, u64)>,
}

impl TierPromotionOpportunityAcc {
    fn new(threshold: u32) -> Self {
        Self { threshold, dispatches: HashMap::new() }
    }

    fn record(&mut self, expression_hash: u64, execution_count: u32, tier: TraceTier, head: &str, timestamp_ns: u64) {
        let entry = self.dispatches.entry(expression_hash)
            .or_insert((0, TraceTier::TreeWalker, head.to_string(), timestamp_ns));
        if execution_count > entry.0 {
            entry.0 = execution_count;
        }
        // Track the highest tier reached
        if (tier as u8) > (entry.1 as u8) {
            entry.1 = tier;
        }
    }

    fn finalize(self) -> Vec<Finding> {
        let mut findings = Vec::new();

        let mut stuck: Vec<_> = self.dispatches.into_iter()
            .filter(|(_, (count, tier, _, _))| {
                *count > self.threshold && *tier == TraceTier::TreeWalker
            })
            .collect();

        stuck.sort_by(|a, b| b.1.0.cmp(&a.1.0));

        for (hash, (count, _, head, first_ts)) in stuck.into_iter().take(20) {
            let msg = format!(
                "Expression `{}` (hash {:#x}): executed {} times, never promoted past TreeWalker",
                head, hash, count,
            );
            findings.push(
                Finding::new(Severity::Info, "tier-promotion-opportunity", msg)
                    .with_time(first_ts, None),
            );
        }

        findings
    }
}

// ── LintPass — orchestrates all accumulators ─────────────────────────────────

struct LintPass {
    branch_imbalance: Option<BranchImbalanceAcc>,
    gc_storm: Option<GcStormAcc>,
    tier_thrash: Option<TierThrashAcc>,
    workpool_saturation: Option<WorkpoolSaturationAcc>,
    sequential_fan_out: Option<SequentialFanOutAcc>,
    gc_pause_outlier: Option<GcPauseOutlierAcc>,
    eval_depth_explosion: Option<EvalDepthExplosionAcc>,
    compilation_latency: Option<CompilationLatencyAcc>,
    workpool_oscillation: Option<WorkpoolOscillationAcc>,
    workpool_floor_pinned: Option<WorkpoolFloorPinnedAcc>,
    workpool_emergency_storm: Option<WorkpoolEmergencyStormAcc>,
    workpool_pressure_dominance: Option<WorkpoolPressureDominanceAcc>,
    workpool_convergence: Option<WorkpoolConvergenceAcc>,
    // Phase A6 lints
    repeated_eval: Option<RepeatedEvalAcc>,
    empty_branch_ratio: Option<EmptyBranchRatioAcc>,
    rule_match_explosion: Option<RuleMatchExplosionAcc>,
    type_inference_miss: Option<TypeInferenceMissAcc>,
    fork_depth_explosion: Option<ForkDepthExplosionAcc>,
    speculative_waste: Option<SpeculativeWasteAcc>,
    gc_allocation_hotspot: Option<GcAllocationHotspotAcc>,
    tier_promotion_opportunity: Option<TierPromotionOpportunityAcc>,
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
            workpool_oscillation: config.is_enabled("workpool-oscillation")
                .then(WorkpoolOscillationAcc::new),
            workpool_floor_pinned: config.is_enabled("workpool-floor-pinned")
                .then(WorkpoolFloorPinnedAcc::new),
            workpool_emergency_storm: config.is_enabled("workpool-emergency-storm")
                .then(WorkpoolEmergencyStormAcc::new),
            workpool_pressure_dominance: config.is_enabled("workpool-pressure-dominance")
                .then(WorkpoolPressureDominanceAcc::new),
            workpool_convergence: config.is_enabled("workpool-convergence")
                .then(WorkpoolConvergenceAcc::new),
            // Phase A6 lints
            repeated_eval: config.is_enabled("repeated-eval")
                .then(|| RepeatedEvalAcc::new(config.repeated_eval_threshold)),
            empty_branch_ratio: config.is_enabled("empty-branch-ratio")
                .then(EmptyBranchRatioAcc::new),
            rule_match_explosion: config.is_enabled("rule-match-explosion")
                .then(|| RuleMatchExplosionAcc::new(config.rule_match_threshold)),
            type_inference_miss: config.is_enabled("type-inference-miss")
                .then(TypeInferenceMissAcc::new),
            fork_depth_explosion: config.is_enabled("fork-depth-explosion")
                .then(|| ForkDepthExplosionAcc::new(config.fork_depth_threshold)),
            speculative_waste: config.is_enabled("speculative-waste")
                .then(SpeculativeWasteAcc::new),
            gc_allocation_hotspot: config.is_enabled("gc-allocation-hotspot")
                .then(GcAllocationHotspotAcc::new),
            tier_promotion_opportunity: config.is_enabled("tier-promotion-opportunity")
                .then(|| TierPromotionOpportunityAcc::new(config.tier_promotion_threshold)),
        }
    }

    fn process(&mut self, event: &trace_format::TraceEvent) {
        let ts = event.timestamp_ns;
        let tid = event.thread_id;
        let seq = event.seq;
        let dur = event.duration_ns.unwrap_or(0);
        let end_ns = ts + dur;

        // G. eval-depth-explosion — checks every event's depth
        if let Some(acc) = &mut self.eval_depth_explosion {
            acc.record(event.depth, ts);
        }

        // S. speculative-waste — tracks wall time on every event
        if let Some(acc) = &mut self.speculative_waste {
            acc.update_wall_time(end_ns);
        }

        // N. repeated-eval — track all timed computation events
        if let Some(acc) = &mut self.repeated_eval {
            if dur > 0 {
                match &event.kind {
                    TraceEventKind::RuleApplication { .. }
                    | TraceEventKind::GroundedOp { .. } => {
                        let input_hash = crate::util::hash_trace_value(&event.input);
                        let head = crate::util::extract_operator_name(&event.input, &event.kind);
                        acc.record(input_hash, &head, ts);
                    }
                    _ => {}
                }
            }
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
                // O. empty-branch-ratio
                if let Some(acc) = &mut self.empty_branch_ratio {
                    let input_hash = crate::util::hash_trace_value(&event.input);
                    let head = crate::util::extract_operator_name(&event.input, &event.kind);
                    acc.record_fork(tid, ts, *branch_count, input_hash, &head);
                }
                // R. fork-depth-explosion
                if let Some(acc) = &mut self.fork_depth_explosion {
                    let head = crate::util::extract_operator_name(&event.input, &event.kind);
                    acc.record_fork(ts, tid, &head);
                }
            }

            TraceEventKind::BranchStart { branch_index, .. } => {
                if let Some(acc) = &mut self.sequential_fan_out {
                    acc.record_branch_start(tid, seq);
                }
                // H7 Stage 2: record for span_id-based correlation.
                if let Some(acc) = &mut self.branch_imbalance {
                    if let Some(span_id) = event.span_id {
                        acc.record_branch_start(ts, tid, *branch_index, span_id);
                    }
                }
            }

            TraceEventKind::BranchEnd { branch_index, result_count } => {
                if let Some(acc) = &mut self.branch_imbalance {
                    acc.record_branch_end(ts, tid, event.duration_ns, *branch_index, event.span_id);
                }
                if let Some(acc) = &mut self.sequential_fan_out {
                    acc.record_branch_end(tid, seq);
                }
                // O. empty-branch-ratio
                if let Some(acc) = &mut self.empty_branch_ratio {
                    acc.record_branch_end(tid, *result_count);
                }
                // R. fork-depth-explosion
                if let Some(acc) = &mut self.fork_depth_explosion {
                    acc.record_branch_end(tid);
                }
                // S. speculative-waste
                if let Some(acc) = &mut self.speculative_waste {
                    acc.record_branch_end(*result_count, dur);
                }
            }

            // ── GC events ──
            TraceEventKind::GcSafepoint { allocation_delta_bytes, .. } => {
                if let Some(acc) = &mut self.gc_storm {
                    acc.record(ts, dur);
                }
                if let Some(acc) = &mut self.gc_pause_outlier {
                    if dur > 0 {
                        acc.record(ts, dur);
                    }
                }
                // T. gc-allocation-hotspot
                if let Some(acc) = &mut self.gc_allocation_hotspot {
                    acc.record(event.depth, *allocation_delta_bytes);
                }
            }

            // ── Rule match events ──
            TraceEventKind::RuleMatchSet { match_count, .. } => {
                // P. rule-match-explosion
                if let Some(acc) = &mut self.rule_match_explosion {
                    let head = crate::util::extract_operator_name(&event.input, &event.kind);
                    acc.record(ts, *match_count, &head);
                }
            }

            // ── Type inference events ──
            TraceEventKind::TypeInference { expression, source, .. } => {
                // Q. type-inference-miss
                if let Some(acc) = &mut self.type_inference_miss {
                    let expr_display = format!("{}", expression);
                    acc.record(ts, &expr_display, source);
                }
            }

            // ── Tier transition events ──
            TraceEventKind::TierDispatch { expression_hash, selected_tier, execution_count } => {
                if let Some(acc) = &mut self.tier_thrash {
                    acc.record_dispatch(tid, ts, *expression_hash, *selected_tier, *execution_count);
                }
                // U. tier-promotion-opportunity
                if let Some(acc) = &mut self.tier_promotion_opportunity {
                    let head = crate::util::extract_operator_name(&event.input, &event.kind);
                    acc.record(*expression_hash, *execution_count, *selected_tier, &head, ts);
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

            // ── WorkPool scale events (new diagnostic lints) ──
            TraceEventKind::WorkPoolScaleEvent {
                action, active_workers_after, min_workers, emergency,
                queue_depth, objective,
                term_throughput, term_queue_depth, term_slab_pressure, term_rss_pressure,
                bp_level, ema_slab_pressure, ema_rss_pressure,
                ..
            } => {
                if let Some(acc) = &mut self.workpool_oscillation {
                    acc.record(ts, action);
                }
                if let Some(acc) = &mut self.workpool_floor_pinned {
                    acc.record(
                        ts, *active_workers_after, *min_workers,
                        *term_throughput, *term_queue_depth, *term_slab_pressure, *term_rss_pressure,
                    );
                }
                if let Some(acc) = &mut self.workpool_emergency_storm {
                    acc.record(ts, *emergency, *bp_level, *ema_slab_pressure, *ema_rss_pressure);
                }
                if let Some(acc) = &mut self.workpool_pressure_dominance {
                    acc.record(
                        ts, action, *queue_depth,
                        *term_throughput, *term_queue_depth, *term_slab_pressure, *term_rss_pressure,
                    );
                }
                if let Some(acc) = &mut self.workpool_convergence {
                    acc.record(ts, action, *objective);
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
        if let Some(acc) = self.workpool_oscillation {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.workpool_floor_pinned {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.workpool_emergency_storm {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.workpool_pressure_dominance {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.workpool_convergence {
            findings.extend(acc.finalize());
        }
        // Phase A6 lints
        if let Some(acc) = self.repeated_eval {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.empty_branch_ratio {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.rule_match_explosion {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.type_inference_miss {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.fork_depth_explosion {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.speculative_waste {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.gc_allocation_hotspot {
            findings.extend(acc.finalize());
        }
        if let Some(acc) = self.tier_promotion_opportunity {
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
        repeated_eval_threshold: 3,
        rule_match_threshold: 10,
        fork_depth_threshold: 3,
        tier_promotion_threshold: 100,
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
