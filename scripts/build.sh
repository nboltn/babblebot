#!/bin/bash

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )/.." > /dev/null && pwd )"
cd "$DIR"

cargo build --release
cp "target/release/babblebot" bin
