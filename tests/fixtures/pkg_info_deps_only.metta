; Package with only dependencies (no exports)

(#package
    (#name "deps-only")
    (#version "1.0.0")
)

(#dependencies
    (#std "^1.0")
    (#core "^2.0")
    (#utils (#path "../utils"))
)
