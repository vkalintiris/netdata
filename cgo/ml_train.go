package main

import "C"

import (
	"fmt"
	"os"
	"time"
)

type MlConfig struct {
	num_samples int
	train_every int

	diff_n   int
	smooth_n int
	lag_n    int
}

func GetMlConfig() *MlConfig {
	num_samples := ConfigGetNum("ml", "num samples to train", 300)
	train_every := ConfigGetNum("ml", "train every secs", 30)

	diff_n := ConfigGetNum("ml", "num samples to diff", 1)
	smooth_n := ConfigGetNum("ml", "num samples to smooth", 3)
	lag_n := ConfigGetNum("ml", "num samples to lag", 5)

	return &MlConfig{
		num_samples: num_samples,
		train_every: train_every,

		diff_n:   diff_n,
		smooth_n: smooth_n,
		lag_n:    lag_n,
	}
}

func WriteInfo(cfg *MlConfig) {
	flags := os.O_APPEND | os.O_CREATE | os.O_WRONLY
	fp, err := os.OpenFile("/tmp/go.log", flags, 0644)
	if err != nil {
		panic(err)
	}
	defer fp.Close()

	localhost := NewLocalHost()
	fmt.Fprintf(fp, "Hello from %s\n", localhost.HostName())
	fmt.Fprintf(fp, "%#v", cfg)

	for _, set := range localhost.Sets() {
		set.ReadLock()
		defer set.UnLock()

		fmt.Fprintf(fp, "\tset: %s, update every: %d, num dims: %d\n",
			set.Name(), set.UpdateEvery(), set.NumDims())
	}
}

//export GoMLTrain
func GoMLTrain() {
	cfg := GetMlConfig()

	for _ = range time.Tick(5 * time.Second) {
		WriteInfo(cfg)
	}
}
