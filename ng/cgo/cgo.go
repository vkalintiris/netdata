package cgo

// #cgo CFLAGS: -I ${SRCDIR}/../../
// #include "cgo.h"
//
import "C"

import (
	"log"
	"os"
	"syscall"
	"unsafe"

	"golang.org/x/sys/unix"
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

func CGoExitCleanly(sig os.Signal) {
	C.error_log_limit_unlimited()
	sigName := unix.SignalName(sig.(syscall.Signal))
	log.Printf("SIGNAL: Received %s. Cleaning up to exit...", sigName)

	C.commands_exit()
	C.netdata_cleanup_and_exit(0)

	os.Exit(0)
}

func CGoSaveDatabase(sig os.Signal) {
	C.error_log_limit_unlimited()
	sigName := unix.SignalName(sig.(syscall.Signal))
	log.Printf("[GO] SIGNAL: Received %s. Saving databases...", sigName)
	C.error_log_limit_reset()

	C.execute_command(C.CMD_SAVE_DATABASE, nil, nil)
}

func CGoReloadHealth(sig os.Signal) {
	C.error_log_limit_unlimited()
	sigName := unix.SignalName(sig.(syscall.Signal))
	log.Printf("[GO] SIGNAL: Received %s. Reloading HEALTH configuration...", sigName)
	C.error_log_limit_reset()

	C.execute_command(C.CMD_RELOAD_HEALTH, nil, nil)
}

func CGoReopenLogs(sig os.Signal) {
	C.error_log_limit_unlimited()
	sigName := unix.SignalName(sig.(syscall.Signal))
	log.Printf("[GO] SIGNAL: Received %s. Reopening all log files...", sigName)
	C.error_log_limit_reset()

	C.execute_command(C.CMD_REOPEN_LOGS, nil, nil)
}

func CGoRrdCalcLabelsUnlink() {
	C.rrdcalc_labels_unlink()
}
