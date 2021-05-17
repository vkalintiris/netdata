package main

import (
	"log"
	"os"
	"os/signal"
	"runtime"
	"syscall"

	"github.com/netdata/netdata/ng/cgo"
)

func setupLogger(path string) *os.File {
	file, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0666)
	if err != nil {
		panic("Could not create Go's log file")
	}

	log.SetOutput(file)

	return file
}

func testConf() {
	conf := cgo.GetNetdataConfig()

	section := "health"
	key := "script to execute on alarm"
	confDir := conf.GetString(section, key, "tsimpa ena arxidi")
	log.Printf("script: %s\n", confDir)

	section = "global"
	key = "process nice level"
	niceness := conf.GetInt(section, key, 5)
	log.Printf("niceness: %d\n", niceness)

	section = "statsd"
	key = "histograms and timers percentile (percentThreshold)"
	percentile := conf.GetFloat(section, key, 0.87654321)
	log.Printf("percentile: %f\n", percentile)

	host := cgo.GetLocalHost()
	log.Printf("host name: %s\n", host.GetName())

	log.Printf("os: '%s'\n", runtime.GOOS)
}

func handleSignals() {
	signal.Ignore(syscall.SIGPIPE)

	signalsToHandle := []os.Signal{
		syscall.SIGINT,
		syscall.SIGQUIT,
		syscall.SIGTERM,
		syscall.SIGUSR1,
		syscall.SIGUSR2,
		syscall.SIGHUP,
	}

	sigCh := make(chan os.Signal)
	signal.Notify(sigCh, signalsToHandle...)

	for {
		switch sig := <-sigCh; sig {
		case syscall.SIGINT, syscall.SIGQUIT, syscall.SIGTERM:
			cgo.CGoExitCleanly(sig)
		case syscall.SIGUSR1:
			cgo.CGoSaveDatabase(sig)
		case syscall.SIGUSR2:
			cgo.CGoReloadHealth(sig)
		case syscall.SIGHUP:
			cgo.CGoReopenLogs(sig)
		default:
			log.Fatalf("Received unknown signal: %s", sig)
		}
	}
}

// TODO: Add go-reaper
func main() {
	switch rc := cgo.CGOMain(os.Args); rc {
	case cgo.CGoMainExitSuccess, cgo.CGoMainExitFailure:
		os.Exit(rc)
	case cgo.CGoMainBlock:
		logFile := setupLogger("/tmp/ng.log")
		defer logFile.Close()

		testConf()

		handleSignals()
	}
}
