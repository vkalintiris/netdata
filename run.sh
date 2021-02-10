#!/usr/bin/env bash

set -exu -o pipefail

find "$HOME/opt/netdata/var/cache/netdata/" -type f -delete

bear make install V=1 -j 8 >/dev/null 2>&1

valgrind "$HOME/opt/netdata/usr/sbin/netdata" -W createdataset=600
