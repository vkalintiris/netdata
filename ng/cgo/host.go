package cgo

// #cgo CFLAGS: -I ${SRCDIR}/../../
// #include "cgo.h"
//
import "C"

import (
	"runtime"
)

type Host struct {
	cptr *C.struct_rrdhost
}

func GetLocalHost() *Host {
	return &Host{cptr: C.localhost}
}

func (h *Host) Lock() {
	runtime.LockOSThread()
	C.__netdata_rwlock_rdlock(&h.cptr.rrdhost_rwlock)
}

func (h *Host) Unlock() {
	C.__netdata_rwlock_unlock(&h.cptr.rrdhost_rwlock)
	runtime.UnlockOSThread()
}

func (h *Host) GetName() string {
	return C.GoString(h.cptr.hostname)
}

func (h *Host) GetAlarms() []*Calc {
	calcs := []*Calc{}

	for alarmCptr := h.cptr.alarms; alarmCptr != nil; alarmCptr = alarmCptr.next {
		calcs = append(calcs, &Calc{alarmCptr})
	}

	return calcs
}
