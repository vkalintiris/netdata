package cgo

// #cgo CFLAGS: -I ${SRCDIR}/../../
// #include "cgo.h"
//
import "C"

import (
	"unsafe"
)

var (
	CGoMainExitSuccess = C.CGO_MAIN_EXIT_SUCCESS
	CGoMainExitFailure = C.CGO_MAIN_EXIT_FAILURE
	CGoMainBlock       = C.CGO_MAIN_BLOCK
)

func CGOMain(args []string) int {
	argc := C.int(len(args))
	argv := make([]*C.char, argc)

	for idx, arg := range args {
		argv[idx] = C.CString(arg)
	}

	argvp := (**C.char)(unsafe.Pointer(&argv[0]))
	return int(C.cgo_main(argc, argvp))
}

func CGoSignalsHandle() {
	C.signals_handle()
}
