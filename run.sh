#!/usr/bin/env bash

set -exu -o pipefail

find "$HOME/opt/netdata/var/cache/netdata/" -type f -delete

bear make install V=1 -j 8

export NUM_SAMPLES=3600
export NUM_DIMS_PER_SAMPLE=8
export DIFF_N=1
export SMOOTH_N=3
export LAG_N=5

valgrind "$HOME/opt/netdata/usr/sbin/netdata" -W createdataset=$NUM_SAMPLES
