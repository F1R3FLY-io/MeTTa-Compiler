----------------------- MODULE InnerColumnReadRefinement -----------------------
EXTENDS TLC

CONSTANTS
  RewriteBeforeEscape,
  UseIdStoreForSpaceMemo

VARIABLES
  variant,
  column,
  idstore,
  escaped,
  read,
  source

Vars == <<variant, column, idstore, escaped, read, source>>

Variants == {"pod", "spaceMemo"}
Sources == {"none", "column", "id"}

TypeOK ==
  /\ RewriteBeforeEscape \in BOOLEAN
  /\ UseIdStoreForSpaceMemo \in BOOLEAN
  /\ variant \in Variants
  /\ column \in {"old", "new"}
  /\ idstore \in BOOLEAN
  /\ escaped \in BOOLEAN
  /\ read \in BOOLEAN
  /\ source \in Sources

Init ==
  /\ variant \in Variants
  /\ column = "old"
  /\ idstore = FALSE
  /\ escaped = FALSE
  /\ read = FALSE
  /\ source = "none"

WriteIdStore ==
  /\ variant = "spaceMemo"
  /\ ~idstore
  /\ idstore' = TRUE
  /\ UNCHANGED <<variant, column, escaped, read, source>>

WriteColumn ==
  /\ variant = "pod"
  /\ column = "old"
  /\ column' = "new"
  /\ UNCHANGED <<variant, idstore, escaped, read, source>>

EscapeHandle ==
  /\ ~escaped
  /\ IF variant = "spaceMemo"
     THEN idstore
     ELSE IF RewriteBeforeEscape THEN column = "new" ELSE TRUE
  /\ escaped' = TRUE
  /\ UNCHANGED <<variant, column, idstore, read, source>>

ReadHandle ==
  /\ escaped
  /\ ~read
  /\ read' = TRUE
  /\ source' =
       IF variant = "spaceMemo" /\ UseIdStoreForSpaceMemo
       THEN "id"
       ELSE "column"
  /\ UNCHANGED <<variant, column, idstore, escaped>>

Done ==
  /\ read
  /\ UNCHANGED Vars

Next ==
  \/ WriteIdStore
  \/ WriteColumn
  \/ EscapeHandle
  \/ ReadHandle
  \/ Done

Spec == Init /\ [][Next]_Vars

NoStaleRead ==
  read =>
    IF source = "id"
    THEN idstore
    ELSE column = "new"

SpaceMemoUsesIdStore ==
  read /\ variant = "spaceMemo" => source = "id"

PODUsesColumn ==
  read /\ variant = "pod" => source = "column"

=============================================================================
