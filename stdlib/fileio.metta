;; fileio module: MeTTa-bisim for hyperon-experimental's `fileio` builtin module.
;;
;; HE source: hyperon-experimental/lib/src/metta/runner/builtin_mods/fileio.metta
;;
;; This module exposes six grounded operations registered in
;; `src/backend/grounded/fileio.rs`:
;;
;;   (file-open! "path" "rwct")     -- open a file, return a FileHandle
;;   (file-read-to-string! $fh)     -- read entire file from cursor as String
;;   (file-write! $fh "content")    -- write a String, return unit
;;   (file-seek! $fh $offset)       -- move read/write cursor to byte offset
;;   (file-read-exact! $fh $nbytes) -- read up to N bytes as String
;;   (file-get-size! $fh)           -- return file size in bytes
;;
;; The grounded ops are dispatched from the grounded registry; this file
;; only declares their type signatures so the type system records them on
;; import.

(@doc file-open!
  (@desc "Function takes path to the file and open options (r, w, c, a, t) both in form of string, creates filehandle and returns it")
  (@params (
    (@param "Filepath (string atom)")
    (@param "Open options (string atom), r - read, w - write, c - create if file doesn't exist, a - append to file, t - truncate file")))
  (@return "Filehandle or error if combination of path and open options is wrong (e.g. file doesn't exist and no 'c' in options; or 'rc' option provided, since 'c' demands for 'w')"))
(: file-open! (-> String String %Undefined%))

(@doc file-read-to-string!
  (@desc "Function takes filehandle provided by file-open! reads its content from current cursor place till the end of file and returns content in form of string.")
  (@params (
    (@param "Filehandle")))
  (@return "File's content"))
(: file-read-to-string! (-> %Undefined% String))

(@doc file-write!
  (@desc "Function takes filehandle provided by file-open!, content to be written (string atom) and puts content into file associated with filehandle")
  (@params (
    (@param "Filehandle")
    (@param "Content (string atom)")))
  (@return "Unit atom"))
(: file-write! (-> %Undefined% String %Undefined%))

(@doc file-seek!
  (@desc "Function takes filehandle provided by file-open! and desired cursor position (number) and sets cursor to provided position")
  (@params (
    (@param "Filehandle")
    (@param "Desired cursor position (number)")))
  (@return "Unit atom"))
(: file-seek! (-> %Undefined% Number %Undefined%))

(@doc file-read-exact!
  (@desc "Function takes filehandle provided by file-open! and desired number of bytes to read (number), reads content of file from current cursor position (number of read bytes <= input number of bytes to read) and returns it in form of string")
  (@params (
    (@param "Filehandle")
    (@param "Number of bytes to read")))
  (@return "File's content"))
(: file-read-exact! (-> %Undefined% Number String))

(@doc file-get-size!
  (@desc "Function takes filehandle provided by file-open! and returns size of file")
  (@params (
    (@param "Filehandle")))
  (@return "Size of file"))
(: file-get-size! (-> %Undefined% Number))
