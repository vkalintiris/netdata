#!/usr/bin/env bash

set -exu -o pipefail

NETDATA_ROOT="$HOME/opt/netdata"

bear make install V=1 -j 8

for num_samples in $(seq 3600 3600 86400); do
    for num_dims in $(seq 2 4 78); do
        for i in $(seq 1 3); do
            export NUM_DIMS_PER_SAMPLE=$num_dims
            export DIFF_N=1
            export SMOOTH_N=3
            export LAG_N=5

            find "$NETDATA_ROOT/var/cache/netdata/" -type f -delete
            "$NETDATA_ROOT/usr/sbin/netdata" -W createdataset=$num_samples
        done
    done
done
