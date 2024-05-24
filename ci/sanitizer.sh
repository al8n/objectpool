#!/bin/bash

set -ex

export ASAN_OPTIONS="detect_odr_violation=0 detect_leaks=0"

# Run address sanitizer with cargo-hack
RUSTFLAGS="-Z sanitizer=address" \
cargo hack test --tests --each-feature --exclude-no-default-features --exclude-all-features

# Run leak sanitizer with cargo-hack
RUSTFLAGS="-Z sanitizer=leak" \
cargo hack test --tests --each-feature --exclude-no-default-features --exclude-all-features

# Run thread sanitizer with cargo-hack
RUSTFLAGS="-Z sanitizer=thread" \
cargo hack -Zbuild-std test --tests --each-feature --exclude-no-default-features --exclude-all-features
