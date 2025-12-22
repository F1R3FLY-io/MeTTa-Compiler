; Complete example _pkg-info.metta manifest
; Uses HE-compatible Atom-Serde format with #-prefixed keys

(#package
    (#name "test-fixture")
    (#version "1.0.0")
    (#description "Test fixture for _pkg-info.metta format")
    (#authors ("Alice" "Bob"))
    (#license "MIT")
    (#repository "https://github.com/example/test-fixture")
    (#keywords ("test" "fixture" "metta"))
)

(#dependencies
    (#std "^1.0")
    (#utils (#path "../utils"))
    (#external (#git "https://github.com/example/external" #tag "v2.0"))
)

(#exports
    (#public (test-func helper-func TestType))
)
