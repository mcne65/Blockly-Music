#!/usr/bin/env bash

# Test Pravega Transaction Coordinator.
# Run before: STREAM=test1 SIZE_SEC=60 scripts/videotestsrc-to-pravega-hls.sh

set -ex
ROOT_DIR=$(readlink -f $(dirname $0)/..)
pushd ${ROOT_DIR}/gst-plugin-pravega
cargo build
ls -lh ${ROOT_DIR}/gst-plugin-pravega/target/debug/*.so
export GST_PLUGIN_PATH=${ROOT_DIR}/gst-plugin-pravega/target/debug:${GST_PLUGIN_PATH}
# log level can be INFO, DEBUG, or LOG (verbose)
export GST_DEBUG=pravegasrc:LOG,mpegtsbase:DEBUG,mpegtspacketizer:DEBUG,h264parse:DEBUG,pravegatc:TRACE,pravegasink:LOG,INFO
export RUST_BACKTRACE=1
PRAVEGA_CONTROLLER=127.0.0.1:9090
STREAM1=${STREAM1:-test1}
STREAM2=${STREAM2:-test2}
FPS=30

gst-launch-1.0 \
-v \
pravegasrc name=src \
  stream=examples/${STREAM1} \
  controller=${PRAVEGA_CONTROLLER} \
  start-mode=timestamp \
  start-pts-at-zero=false \
! identity silent=false \
! decodebin \
! identity silent=false \
! x264enc key-int-max=${FPS} speed-preset=ultrafast bitrate=200 \
! identity silent=false \
! mpegtsmux \
! identity silent=false \
! pravegatc \
! identity silent=false \
! pravegasink \
  stream=examples/${STREAM2} \
  controller=${PRAVEGA_CONTROLLER} \
  timestamp-mode=tai \
  sync=false \
|& tee /tmp/pravegatc2.log
