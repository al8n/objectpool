#!/bin/bash
set -e

rustup toolchain install nightly --component miri
rustup override set nightly

export MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-disable-isolation -Zmiri-symbolic-alignment-check"

cargo miri test --tests
