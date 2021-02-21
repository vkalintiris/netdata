package main

import "C"

import (
	"fmt"
	"os"
)

//export GoMLTrain
func GoMLTrain() {
	flags := os.O_APPEND | os.O_CREATE | os.O_WRONLY
	fp, err := os.OpenFile("/tmp/go.log", flags, 0644)
	if err != nil {
		panic(err)
	}
	defer fp.Close()

	localhost := NewLocalHost()

	fmt.Fprintf(fp, "Hello from %s\n", localhost.HostName())
	for _, set := range localhost.Sets() {
		fmt.Fprintf(fp, "\tset name %s\n", set.Name())
	}
}
