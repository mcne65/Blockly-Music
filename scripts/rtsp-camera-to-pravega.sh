#!/usr/bin/env bash

#
# Copyright (c) Dell Inc., or its subsidiaries. All Rights Reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#

# Record video from an RTSP camera and write to Pravega.
# Note: Using rtsp-camera-to-pravega-python.sh is preferred.

set -ex

ROOT_DIR=$(readlink -f $(dirname $0)/..)
LOG_FILE=/tmp/rtsp-camera-to-pravega.log
pushd ${ROOT_DIR}/gst-plugin-pravega
cargo build
export GST_PLUGIN_PATH=${ROOT_DIR}/target/debug:${GST_PLUGIN_PATH}
# log level can be INFO, DEBUG, or LOG (verbose)
export GST_DEBUG=pravegasink:DEBUG,basesink:INFO,rtspsrc:INFO,rtpbin:INFO,rtpsession:INFO,rtpjitterbuffer:INFO
export RUST_BACKTRACE=1
PRAVEGA_CONTROLLER_URI=${PRAVEGA_CONTROLLER_URI:-tcp://127.0.0.1:9090}
PRAVEGA_SCOPE=${PRAVEGA_SCOPE:-examples}
ALLOW_CREATE_SCOPE=${ALLOW_CREATE_SCOPE:-true}
PRAVEGA_STREAM=${PRAVEGA_STREAM:-rtsp1}
CAMERA_USER=${CAMERA_USER:-admin}
CAMERA_IP=${CAMERA_IP:-192.168.1.102}
CAMERA_PORT=${CAMERA_PORT:-554}

gst-launch-1.0 \
-v \
--eos-on-shutdown \
rtspsrc \
  "location=rtsp://${CAMERA_USER}:${CAMERA_PASSWORD:?Required environment variable not set}@${CAMERA_IP}:${CAMERA_PORT}/cam/realmonitor?channel=1&subtype=0" \
  buffer-mode=none \
  drop-messages-interval=0 \
  drop-on-latency=true \
  latency=2000 \
  ntp-sync=true \
  ntp-time-source=running-time \
  rtcp-sync-send-time=false \
! rtph264depay \
! h264parse \
! "video/x-h264,alignment=au" \
! mpegtsmux \
! queue max-size-buffers=0 max-size-bytes=10485760 max-size-time=0 silent=true leaky=downstream \
! pravegasink \
  allow-create-scope=${ALLOW_CREATE_SCOPE} \
  controller=${PRAVEGA_CONTROLLER_URI} \
  stream=${PRAVEGA_SCOPE}/${PRAVEGA_STREAM} \
  sync=false \
  timestamp-mode=ntp \
$* |& tee ${LOG_FILE}
