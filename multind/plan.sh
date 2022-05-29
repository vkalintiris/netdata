#!/usr/bin/env bash

set -xu -o pipefail

./driver.py

timeout -v -k 300 -s INT 15 ~/opt/tk2/netdata/usr/sbin/netdata -D -c ./nd0.conf
date
sleep 15

timeout -v -k 300 -s INT 15 ~/opt/tk2/netdata/usr/sbin/netdata -D -c ./nd0.conf
date
sleep 15

timeout -v -k 300 -s INT 15 ~/opt/tk2/netdata/usr/sbin/netdata -D -c ./nd0.conf
date
sleep 15

timeout -v -k 300 -s INT 15 ~/opt/tk2/netdata/usr/sbin/netdata -D -c ./nd0.conf
date
sleep 15

timeout -v -k 300 -s INT 15 ~/opt/tk2/netdata/usr/sbin/netdata -D -c ./nd0.conf
date
