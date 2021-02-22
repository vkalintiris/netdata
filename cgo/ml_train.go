package main

import "C"

import (
	"log"
	"os"
	"time"
)

type MlConfig struct {
	NumSamples int
	TrainEvery time.Duration

	DiffN   int
	SmoothN int
	LagN    int
}

func NewMlConfig() *MlConfig {
	var mlc MlConfig

	mlc.NumSamples = ConfigGetNum("ml", "num samples to train", 300)
	mlc.TrainEvery = time.Duration(ConfigGetNum("ml", "train every secs", 30)) * time.Second

	mlc.DiffN = ConfigGetNum("ml", "num samples to diff", 1)
	mlc.SmoothN = ConfigGetNum("ml", "num samples to smooth", 3)
	mlc.LagN = ConfigGetNum("ml", "num samples to lag", 5)

	return &mlc
}

type MlChart struct {
	Config *MlConfig

	Set           RrdSet
	Name          string
	LastTrainedAt time.Time
}

func (chart *MlChart) ShouldTrain() bool {
	if chart.Set.NumDims() == 0 {
		return false
	}

	if chart.Set.UpdateEvery() != 1 {
		return false
	}

	elapsed := time.Now().Sub(chart.LastTrainedAt)
	return elapsed >= chart.Config.TrainEvery
}

func (chart *MlChart) Train(mlc *MlConfig) bool {
	if !chart.ShouldTrain() {
		return false
	}

	log.Printf("Training %s\n\t(LTA: %s, p: %p)", chart.Name, chart.LastTrainedAt, chart)
	chart.LastTrainedAt = time.Now()
	return true
}

type MlInfo struct {
	Config *MlConfig
	Charts map[string]*MlChart
}

func NewMlInfo() *MlInfo {
	return &MlInfo{Config: NewMlConfig(), Charts: map[string]*MlChart{}}
}

func (mli *MlInfo) CollectCharts() {
	localhost := NewLocalHost()
	now := time.Now()

	for _, set := range localhost.Sets() {
		set.ReadLock()
		defer set.UnLock()

		name := set.Name()
		if name != "system.cpu" {
			continue
		}

		if _, ok := mli.Charts[name]; !ok {
			log.Printf("Adding new chart %s\n", name)
			mli.Charts[name] = &MlChart{mli.Config, set, name, now}
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
