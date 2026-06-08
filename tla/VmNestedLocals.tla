---- MODULE VmNestedLocals ----

EXTENDS FiniteSets

CONSTANTS IncludePreEval,
          IncludeDispatchRhs,
          IncludeRuleMatches,
          IncludeSavedBindings,
          IncludeCombos,
          IncludeOutcomes

VARIABLES phase, rooted, freed

PreEvalLocals == {"pre_eval_expr", "pre_eval_item", "pre_eval_result"}
DispatchLocals == {"dispatch_rhs"}
RuleMatchLocals == {"rule_match_rhs", "rule_match_template", "rule_match_binding"}
SavedBindingLocals == {"saved_binding"}
ComboLocals == {"combo_expr", "combo_binding"}
OutcomeLocals == {"accumulated_outcome"}

VM_LOCALS ==
    PreEvalLocals \cup DispatchLocals \cup RuleMatchLocals \cup
    SavedBindingLocals \cup ComboLocals \cup OutcomeLocals

ConfiguredRoots ==
    (IF IncludePreEval THEN PreEvalLocals ELSE {}) \cup
    (IF IncludeDispatchRhs THEN DispatchLocals ELSE {}) \cup
    (IF IncludeRuleMatches THEN RuleMatchLocals ELSE {}) \cup
    (IF IncludeSavedBindings THEN SavedBindingLocals ELSE {}) \cup
    (IF IncludeCombos THEN ComboLocals ELSE {}) \cup
    (IF IncludeOutcomes THEN OutcomeLocals ELSE {})

Init ==
  /\ phase = "start"
  /\ rooted = {}
  /\ freed = {}

BuildRoots ==
  /\ phase = "start"
  /\ rooted' = ConfiguredRoots
  /\ freed' = freed
  /\ phase' = "rooted"

Sweep ==
  /\ phase = "rooted"
  /\ freed' = VM_LOCALS \ rooted
  /\ rooted' = rooted
  /\ phase' = "swept"

Done ==
  /\ phase = "swept"
  /\ UNCHANGED <<phase, rooted, freed>>

Next == BuildRoots \/ Sweep \/ Done

Spec == Init /\ [][Next]_<<phase, rooted, freed>>

TypeOK ==
  /\ phase \in {"start", "rooted", "swept"}
  /\ rooted \subseteq VM_LOCALS
  /\ freed \subseteq VM_LOCALS

NoLiveVmLocalFreed ==
  freed = {}

====
