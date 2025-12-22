; Full _pkg-info.metta manifest with all fields populated

(#package
    (#name "full-featured-module")
    (#version "2.5.1")
    (#description "A fully-featured module demonstrating all package manifest fields")
    (#authors ("Primary Author <primary@example.com>" "Contributor <contrib@example.com>"))
    (#license "Apache-2.0")
    (#repository "https://github.com/example/full-featured")
    (#documentation "https://docs.example.com/full-featured")
    (#homepage "https://example.com/full-featured")
    (#keywords ("metta" "demo" "complete"))
    (#categories ("utilities" "testing"))
)

(#dependencies
    ; Simple version constraint
    (#core "^1.0")
    (#std "~2.0")
    (#minimum ">=1.5.0")
    (#exact "=3.0.0")

    ; Path dependency
    (#local-lib (#path "../local-lib"))

    ; Git dependencies
    (#git-tag (#git "https://github.com/example/repo1" #tag "v1.0.0"))
    (#git-branch (#git "https://github.com/example/repo2" #branch "develop"))
    (#git-rev (#git "https://github.com/example/repo3" #rev "abc123def456"))

    ; Dependency with version and features
    (#featured (#version "1.0" #features ("feature1" "feature2")))

    ; Optional dependency
    (#optional-dep (#version "1.0" #optional True))
)

(#exports
    (#public (
        public-function
        another-public-fn
        PublicType
        CONSTANT_VALUE
    ))
)
