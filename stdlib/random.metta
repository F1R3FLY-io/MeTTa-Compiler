;; random module: MeTTa-bisim for hyperon-experimental's `random` builtin module.
;;
;; HE source: hyperon-experimental/lib/src/metta/runner/builtin_mods/random.metta
;;
;; This module exposes six grounded operations registered in
;; `src/backend/grounded/random.rs`:
;;
;;   (new-random-generator $seed)        -- create a RandomGenerator (StdRng) seeded from $seed
;;   (random-int $rng $start $end)       -- random int in [$start, $end)
;;   (random-float $rng $start $end)     -- random float in [$start, $end)
;;   (set-random-seed $rng $seed)        -- reseed the generator deterministically
;;   (reset-random-generator $rng)       -- reseed from a non-deterministic OS source
;;   (flip)                              -- random Bool
;;
;; This file declares only the type signatures and `@doc`s. The
;; implementations live in src/backend/grounded/random.rs.

(@doc random-int
  (@desc "Returns random int number from range defined by two numbers (second and third argument)")
  (@params (
    (@param "Random number generator instance")
    (@param "Range start")
    (@param "Range end")))
  (@return "Random int number from defined range"))
(: random-int (-> %Undefined% Number Number Number))

(@doc random-float
  (@desc "Returns random float number from range defined by two numbers (second and third argument)")
  (@params (
    (@param "Random number generator instance")
    (@param "Range start")
    (@param "Range end")))
  (@return "Random float number from defined range"))
(: random-float (-> %Undefined% Number Number Number))

(@doc set-random-seed
  (@desc "Sets a new seed (second argument) for random number generator (first argument)")
  (@params (
    (@param "Random number generator instance")
    (@param "Seed")))
  (@return "Unit atom"))
(: set-random-seed (-> %Undefined% Number %Undefined%))

(@doc new-random-generator
  (@desc "Creates new random number generator instance using seed as input (first argument)")
  (@params (
    (@param "Seed")))
  (@return "Instance of random number generator"))
(: new-random-generator (-> Number %Undefined%))

(@doc reset-random-generator
  (@desc "Resets instance of random number generator (first argument) to its default behavior (StdRng::from_os_rng())")
  (@params (
    (@param "Random number generator instance")))
  (@return "Random number generator instance with default behavior"))
(: reset-random-generator (-> %Undefined% %Undefined%))

(@doc flip
  (@desc "Random boolean value")
  (@params ())
  (@return "Returns uniformly distributed random boolean value"))
(: flip (-> Bool))
