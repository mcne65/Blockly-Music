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

set -ex

ROOT_DIR=$(readlink -f $(dirname $0)/..)
CONTAINER_NAME=$(basename -s .sh $0)
export GST_DEBUG="pravegasrc:INFO,basesrc:INFO,mpegtsbase:INFO,mpegtspacketizer:INFO"
export RUST_BACKTRACE=1
export RUST_LOG=info
PRAVEGA_CONTROLLER_URI=${PRAVEGA_CONTROLLER_URI:-192.168.1.123:9090}

docker stop ${CONTAINER_NAME} || true

docker run -d --rm \
--name ${CONTAINER_NAME} \
-p 3030:3030 \
-e GST_DEBUG \
-e RUST_BACKTRACE \
-e RUST_LOG \
--workdir /usr/src/gstreamer-pravega/pravega-video-server \
pravega/gstreamer:pravega-dev \
pravega-video-server \
--controller ${PRAVEGA_CONTROLLER_URI} \
$*

docker logs --follow ${CONTAINER_NAME}
