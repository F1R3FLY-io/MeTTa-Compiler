; Module that uses force-eval to add a dynamic rule
(= (static-rule) 42)
!(add-atom &self (= (dynamic-rule) 99))
(: static-rule (-> Long))
