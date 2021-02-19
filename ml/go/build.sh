#!/usr/bin/env bash

set -exu -o pipefail

go build -o mlgo.a -buildmode=c-archive mlgo.go
