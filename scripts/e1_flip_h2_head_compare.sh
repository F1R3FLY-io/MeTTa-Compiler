#!/usr/bin/env bash
# Retired H2 experiment wrapper.
#
# This script used to compare an old commit's dedicated-GC on/off environment
# gate. That gate has been removed from current production code; leaving the old
# command body runnable would make the validation surface look configurable when
# it is not. Use `git show 4c6df1d^:scripts/e1_flip_h2_head_compare.sh` if the
# historical experiment must be reconstructed against the old commit.
set -euo pipefail

echo "e1_flip_h2_head_compare.sh is retired: dedicated index GC is no longer env-gated."
echo "Use scripts/e1_flip_discriminator.sh for the current default-dedicated check."
