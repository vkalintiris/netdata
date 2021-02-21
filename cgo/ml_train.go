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
		set.ReadLock()
		defer set.UnLock()

		fmt.Fprintf(fp, "\tset: %s, update every: %d, num dims: %d\n",
			set.Name(), set.UpdateEvery(), set.NumDims())
	}
}
