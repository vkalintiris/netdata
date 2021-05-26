package health

import (
	"log"
	"time"

	"github.com/netdata/netdata/ng/cgo"
)

func HandleHealth() {
	for {
		host := cgo.GetLocalHost()
		host.Lock()

		alarms := host.GetAlarms()
		log.Printf("[A] num alarms: %d", len(alarms))

		host.Unlock()

		time.Sleep(1 * time.Second)
		log.Printf("GVD")
	}
}
