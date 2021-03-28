#!/usr/bin/env bash

# Write H264 to Pravega without an MPEG Transport Stream.
# This can be played back using pravega-to-screen.sh.

set -ex
ROOT_DIR=$(readlink -f $(dirname $0)/..)
pushd ${ROOT_DIR}/gst-plugin-pravega
cargo build
ls -lh ${ROOT_DIR}/gst-plugin-pravega/target/debug/*.so
export GST_PLUGIN_PATH=${ROOT_DIR}/gst-plugin-pravega/target/debug:${GST_PLUGIN_PATH}
# log level can be INFO, DEBUG, or LOG (verbose)
export GST_DEBUG=x264enc:LOG,pravegasink:LOG,basesink:INFO
export GST_DEBUG_DUMP_DOT_DIR=/tmp/gst-dot/videotestsrc-to-pravega-h264stream
export RUST_BACKTRACE=1
export TZ=UTC
mkdir -p ${GST_DEBUG_DUMP_DOT_DIR}
STREAM=${STREAM:-test1}
SIZE_SEC=${SIZE_SEC:-60}
FPS=30
KEY_FRAME_INTERVAL=$((5*$FPS))

gst-launch-1.0 \
-v \
videotestsrc name=src is-live=false do-timestamp=true num-buffers=$(($SIZE_SEC*$FPS)) \
! "video/x-raw,format=YUY2,width=640,height=480,framerate=${FPS}/1" \
! videoconvert \
! clockoverlay "font-desc=Sans 48px" "time-format=%F %T" shaded-background=true \
! timeoverlay valignment=bottom "font-desc=Sans 48px" shaded-background=true \
! videoconvert \
! x264enc key-int-max=${KEY_FRAME_INTERVAL} tune=zerolatency speed-preset=medium bitrate=500 \
! "video/x-h264,stream-format=byte-stream,profile=main" \
! pravegasink stream=examples/${STREAM} controller=127.0.0.1:9090 seal=false sync=false \
|& tee /tmp/videotestsrc-to-pravega-h264stream.log
