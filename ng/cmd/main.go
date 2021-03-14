package main

import (
	"os"

	"github.com/netdata/netdata/ng/cgo"
)

func main() {
	switch rc := cgo.CGOMain(os.Args); rc {
	case cgo.CGoMainExitSuccess, cgo.CGoMainExitFailure:
		os.Exit(rc)
	case cgo.CGoMainBlock:
		cgo.CGoSignalsHandle()
	}
}
