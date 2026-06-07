---- MODULE NodeEdgeCompleteness ----
EXTENDS FiniteSets

CONSTANTS IncludeInline, IncludeSide, IncludeSpace, TargetKind

VARIABLES marked, swept, phase

Addr == {"root", "child", "other"}
Root == {"root"}

SemanticEdge(parent, child) ==
  /\ parent = "root"
  /\ child = "child"
  /\ TargetKind \in {"inline", "side", "space"}

ReaderEdge(parent, child) ==
  /\ parent = "root"
  /\ child = "child"
  /\ \/ /\ TargetKind = "inline"
        /\ IncludeInline
     \/ /\ TargetKind = "side"
        /\ IncludeSide
     \/ /\ TargetKind = "space"
        /\ IncludeSpace

Reachable ==
  Root \cup { child \in Addr : \E parent \in Root : SemanticEdge(parent, child) }

Init ==
  /\ marked = {}
  /\ swept = {}
  /\ phase = "start"

MarkRoots ==
  /\ phase = "start"
  /\ marked' = marked \cup Root
  /\ swept' = swept
  /\ phase' = "roots_marked"

MarkChildren ==
  /\ phase = "roots_marked"
  /\ marked' =
      marked \cup { child \in Addr : \E parent \in marked : ReaderEdge(parent, child) }
  /\ swept' = swept
  /\ phase' = "children_marked"

Sweep ==
  /\ phase = "children_marked"
  /\ swept' = { addr \in Addr : addr \notin marked }
  /\ marked' = marked
  /\ phase' = "swept"

Done ==
  /\ phase = "swept"
  /\ UNCHANGED <<marked, swept, phase>>

Next == MarkRoots \/ MarkChildren \/ Sweep \/ Done

Spec == Init /\ [][Next]_<<marked, swept, phase>>

NoReachableFreed == swept \cap Reachable = {}

====
