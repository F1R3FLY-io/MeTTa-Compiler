// PathMap-based Pattern Matching Demo
// This demonstrates how MeTTa expressions compile to Rholang PathMap operations

// Add atoms to a space (uses PathMap union)
(add-atom $mySpace (= $x 42))
(add-atom $mySpace (= $y "hello"))

// Pattern matching with PathMap restriction
(match $mySpace $x $x)

// Remove atom (uses PathMap filtering)
(rem-atom $mySpace $x)

// Knowledge base with PathMap storage
(! (= $fact1 100) .)
(! (= $fact2 200) .)

// Query with PathMap restriction
(? $fact1 .)
