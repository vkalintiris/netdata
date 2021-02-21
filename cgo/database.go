package main

// #cgo LDFLAGS: -Wl,--unresolved-symbols=ignore-all
// #include "bindings/database.h"
import "C"

type RrdHost struct {
	rrdhostp C.RRDHOSTP
}

var Localhost = RrdHost{C.localhost}

func (host *RrdHost) HostName() string {
	return C.GoString(C.rrdhostp_hostname(host.rrdhostp))
}

func main() {}
