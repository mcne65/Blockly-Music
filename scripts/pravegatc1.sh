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
STREAM=${STREAM:-test1}
SIZE_SEC=1
FPS=10

gst-launch-1.0 \
-v \
videotestsrc name=src is-live=false do-timestamp=true num-buffers=$(($SIZE_SEC*$FPS)) \
! "video/x-raw,format=YUY2,width=320,height=200,framerate=${FPS}/1" \
! videoconvert \
! clockoverlay "font-desc=Sans 48px" "time-format=%F %T" shaded-background=true \
! timeoverlay valignment=bottom "font-desc=Sans 48px" shaded-background=true \
! videoconvert \
! x264enc key-int-max=${FPS} speed-preset=ultrafast bitrate=200 \
! mpegtsmux alignment=-1 \
! pravegatc \
! pravegasink stream=examples/${STREAM} controller=127.0.0.1:9090 seal=false sync=false
