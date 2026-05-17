;; json module: MeTTa-bisim for hyperon-experimental's `json` builtin module.
;;
;; HE source: hyperon-experimental/lib/src/metta/runner/builtin_mods/json.metta
;;
;; Provides three rule-based functions over a "dict-space" (a space whose
;; atoms are (key value) pairs):
;;
;;   (get-value $space $key)    -- value tied to $key in $space
;;   (get-keys $space)          -- all keys in $space (nondeterministic)
;;   (dict-space $expr)         -- build a space from a tuple of pairs
;;
;; The two grounded functions `json-encode` and `json-decode` are
;; registered in `src/backend/grounded/json.rs` and dispatched from the
;; grounded registry; their type signatures are declared here so that the
;; type system records them on import.

(@doc get-value
  (@desc "Function takes space and key as input, checks if space contains key-value pairs in form of (key value) and returns value tied to the input key")
  (@params (
    (@param "Space")
    (@param "Key")))
  (@return "Value which tied to input key, empty if no such key in space"))
(: get-value (-> Grounded Atom %Undefined%))
(= (get-value $dictspace $key) (match $dictspace ($key $value) $value))

(@doc get-keys
  (@desc "Function takes space and returns all keys from (<key> <value>) tuples in space")
  (@params (
    (@param "Space")))
  (@return "All keys in the input space"))
(: get-keys (-> Grounded Atom))
(= (get-keys $dictspace) (match $dictspace ($key $value) $key))

(@doc dict-space
  (@desc "Function takes key-value pairs in form of expression as input, creates space and adds key-value pairs into it")
  (@params (
    (@param "Expression")))
  (@return "Space"))
(: dict-space (-> Expression Grounded))
(= (dict-space $expr)
   (let* (($dictspace (new-space))
          ($res (add-reducts $dictspace $expr)))
        $dictspace))

(@doc json-encode
  (@desc "Function takes atom as an input and encodes it to json-string. Atom could be a string, number, expression, space and combination of those")
  (@params (
    (@param "Input atom")))
  (@return "Json string"))
(: json-encode (-> Atom String))

(@doc json-decode
  (@desc "Function takes json string as an input and decodes it to the metta objects (list to expression, dictionary to space which will contain key-value pairs in form of (key value), string to string, number to number)")
  (@params (
    (@param "Json string")))
  (@return "Metta object"))
(: json-decode (-> String Atom))
