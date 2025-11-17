#include "tree_sitter/parser.h"

// Minimal scanner for MeTTa grammar
// This grammar does not require external scanning,
// so this file provides stub implementations.

void *tree_sitter_metta_external_scanner_create(void) {
  return NULL;
}

void tree_sitter_metta_external_scanner_destroy(void *payload) {
  // No-op
}

unsigned tree_sitter_metta_external_scanner_serialize(void *payload, char *buffer) {
  return 0;
}

void tree_sitter_metta_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  // No-op
}

bool tree_sitter_metta_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  return false;
}
