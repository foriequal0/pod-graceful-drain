package core

import (
	"errors"
	"flag"
	"time"
)

type PodGracefulDrainConfig struct {
	DeleteAfter     time.Duration
	NoDenyAdmission bool
	IgnoreError     bool
}

func (c *PodGracefulDrainConfig) BindFlags(fs *flag.FlagSet) {
	fs.DurationVar(&c.DeleteAfter, "delete-after", 90*time.Second, "Amount of time that a pod is deleted after a denial of an admission")
	fs.BoolVar(&c.NoDenyAdmission, "no-deny-admission", false, "Delay a pod deletion by only delaying an admission without denying it")
	fs.BoolVar(&c.IgnoreError, "ignore-error", true, "Allow pod deletion even if there were errors during the pod deletion interception")
}

func (c *PodGracefulDrainConfig) Validate() error {
	if c.DeleteAfter < time.Duration(0) {
		return errors.New("deletion delay cannot be less than 0 (time travelling?)")
	}

	if !c.NoDenyAdmission {
		if c.DeleteAfter == time.Duration(0) {
			return errors.New("deletion delay cannot be 0 when you choose to deny admissions")
		}
	}

	return nil
}
