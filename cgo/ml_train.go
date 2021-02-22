package main

import "C"

import (
	"log"
	"os"
	"time"
)

type MlConfig struct {
	NumSamples int
	TrainEvery int

	DiffN   int
	SmoothN int
	LagN    int
}

func NewMlConfig() *MlConfig {
	var mlc MlConfig

	mlc.NumSamples = ConfigGetNum("ml", "num samples to train", 300)
	mlc.TrainEvery = ConfigGetNum("ml", "train every secs", 30)

	mlc.DiffN = ConfigGetNum("ml", "num samples to diff", 1)
	mlc.SmoothN = ConfigGetNum("ml", "num samples to smooth", 3)
	mlc.LagN = ConfigGetNum("ml", "num samples to lag", 5)

	return &mlc
}

type MlChart struct {
	Config *MlConfig

	Set           RrdSet
	Name          string
	LastTrainedAt int
}

func (chart *MlChart) Train(mlc *MlConfig) bool {
	if chart.Set.NumDims() == 0 || chart.Set.UpdateEvery() != 1 {
		return false
	}

	return true
}

type MlInfo struct {
	Config *MlConfig
	Charts map[string]MlChart
}

func NewMlInfo() *MlInfo {
	return &MlInfo{Config: NewMlConfig(), Charts: map[string]MlChart{}}
}

func (mli *MlInfo) CollectCharts() {
	localhost := NewLocalHost()

	for _, set := range localhost.Sets() {
		set.ReadLock()
		defer set.UnLock()

		name := set.Name()
		if chart, ok := mli.Charts[name]; ok {
			log.Printf("Found chart %s\n", chart.Name)
		} else {
			log.Printf("Adding new chart %s\n", name)
			mli.Charts[name] = MlChart{mli.Config, set, name, 0}
		}
	}
}

func TrainModels(mli *MlInfo) {
	mli.CollectCharts()

	for _, chart := range mli.Charts {
		if chart.Train(mli.Config) == false {
			log.Printf("Could not train: %+v\n", chart)
		}
	}
}

//export GoMLTrain
func GoMLTrain() {
	flags := os.O_APPEND | os.O_CREATE | os.O_WRONLY
	fp, err := os.OpenFile("/tmp/go.log", flags, 0664)
	if err != nil {
		log.Fatal(err)
	}
	defer fp.Close()
	log.SetOutput(fp)

	mli := NewMlInfo()

	for _ = range time.Tick(5 * time.Second) {
		TrainModels(mli)
	}
}
