package cgo

// #cgo CFLAGS: -I ${SRCDIR}/../../
// #include "cgo.h"
//
import "C"

type Host struct {
	cptr *C.struct_rrdhost
}

func GetLocalHost() *Host {
	return &Host{cptr: C.localhost}
}

func (h *Host) GetName() string {
	return C.GoString(h.cptr.hostname)
}
