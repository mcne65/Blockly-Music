#!/usr/bin/env bash

set -ex
ROOT_DIR=$(readlink -f $(dirname $0)/..)
pushd ${ROOT_DIR}/gst-plugin-pravega
cargo build
ls -lh ${ROOT_DIR}/gst-plugin-pravega/target/debug/*.so
export GST_PLUGIN_PATH=${ROOT_DIR}/gst-plugin-pravega/target/debug:${GST_PLUGIN_PATH}
# log level can be INFO, DEBUG, or LOG (verbose)
export GST_DEBUG=pravegatc:TRACE,pravegasink:FIXME,basesink:FIXME
export RUST_BACKTRACE=1
PRAVEGA_CONTROLLER=127.0.0.1:9090
STREAM1=${STREAM:-test1}
STREAM2=${STREAM:-test2}

gst-launch-1.0 \
-v \
pravegasrc \
  stream=examples/${STREAM1} \
  controller=${PRAVEGA_CONTROLLER} \
  start-pts-at-zero=false \
! pravegatc \
! pravegasink \
  stream=examples/${STREAM2} \
  controller=${PRAVEGA_CONTROLLER} \
  timestamp-mode=tai \
  sync=false
