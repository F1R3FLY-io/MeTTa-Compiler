; Create multiple specialized knowledge spaces
!(bind! &entities (new-space))
!(bind! &relations (new-space))
!(bind! &rules (new-space))
!(bind! &cache (new-space))

; Load complex relational data
!(add-atom &entities (entity person alice (properties (age 25) (city NYC))))
!(add-atom &entities (entity person bob (properties (age 30) (city LA))))
!(add-atom &entities (entity person carol (properties (age 35) (city NYC))))
!(add-atom &entities (entity company acme (properties (size large) (city SF))))
!(add-atom &entities (entity company beta (properties (size small) (city NYC))))

!(add-atom &relations (works alice acme))
!(add-atom &relations (works bob beta))
!(add-atom &relations (friends alice carol))
!(add-atom &relations (located acme SF))
!(add-atom &relations (located beta NYC))

; Complex inference rules with cross-space operations
!(add-atom &rules (rule 
    (colleague $x $y)
    (and (works $x $company) (works $y $company) (different $x $y))))

!(add-atom &rules (rule
    (same-city $x $y)
    (and (entity person $x (properties $props1))
        (entity person $y (properties $props2))
        (has-property $props1 (city $city))
        (has-property $props2 (city $city))
        (different $x $y))))

; Cross-space query processor with caching
(= (cached-query $query $result)
    (let $cached (match &cache ($query cached $result) $result)
      (if (== $cached empty)
          (let $computed (compute-query $query)
            (progn 
              (add-atom &cache ($query cached $computed))
              $computed))
          $cached)))

(= (compute-query (colleague $x $y))
    (match &relations (works $x $company)
      (match &relations (works $y $company)
          (if (== $x $y) empty (colleague $x $y)))))

(= (compute-query (same-city $x $y))
    (match &entities (entity person $x (properties $props1))
      (match &entities (entity person $y (properties $props2))
          (extract-cities $props1 $props2 $x $y))))

(= (extract-cities $props1 $props2 $x $y)
    (has-property $props1 (city $city))
    (has-property $props2 (city $city))
    (if (== $x $y) empty (same-city $x $y)))

(= (has-property ((city $c) . $_) (city $c)) True)
(= (has-property ((age $_) . $rest) $prop)
    (has-property $rest $prop))
(= (has-property ((size $_) . $rest) $prop)
    (has-property $rest $prop))
(= (has-property () $_) empty)

; Stress the system with complex cross-space queries
!(cached-query (colleague $x $y) $result)
!(cached-query (same-city $x $y) $result)
!(match &entities (entity $type $name (properties $props)) ($type $name))
!(cached-query (colleague alice bob) $result)
!(cached-query (same-city alice carol) $result)
!(match &relations (works $person $company) ($person $company))