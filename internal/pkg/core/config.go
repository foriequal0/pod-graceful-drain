package core

import (
	"errors"
	"flag"
	"time"
)

type PodGracefulDrainConfig struct {
	DeleteAfter     time.Duration
	NoDenyAdmission bool
	AdmissionDelay  time.Duration
	IgnoreError     bool
}

func (c *PodGracefulDrainConfig) BindFlags(fs *flag.FlagSet) {
	fs.DurationVar(&c.DeleteAfter, "delete-after", 90*time.Second, "Amount of time that a pod is deleted after a denial of an admission")
	fs.BoolVar(&c.NoDenyAdmission, "no-deny-admission", false, "Delay a pod deletion by only delaying an admission without denying it")
	fs.DurationVar(&c.AdmissionDelay, "admission-delay", 25*time.Second, "Amount of time that an admission takes")
	fs.BoolVar(&c.IgnoreError, "ignore-error", true, "Allow pod deletion even if there were errors during the pod deletion interception")
}

func (c *PodGracefulDrainConfig) GetDrainDuration() time.Duration {
	if c.NoDenyAdmission || c.AdmissionDelay > c.DeleteAfter {
		return c.AdmissionDelay
	} else {
		return c.DeleteAfter
	}
}

func (c *PodGracefulDrainConfig) Validate() error {
	if c.DeleteAfter < time.Duration(0) {
		return errors.New("deletion delay cannot be less than 0 (time travelling?)")
	}
	if c.AdmissionDelay < time.Duration(0) {
		return errors.New("admission delay cannot be less than 0 (time travelling?)")
	}
	if c.AdmissionDelay > 30*time.Second {
		return errors.New("admission delay cannot be greater than 30s since the maximum value of 30 * timeoutSeconds")
	}

	if c.NoDenyAdmission {
		if c.AdmissionDelay == time.Duration(0) {
			return errors.New("admission delay cannot be 0 when you choose not to deny admissions")
		}
	} else {
		if c.DeleteAfter == time.Duration(0) {
			return errors.New("deletion delay cannot be 0 when you choose to deny admissions")
		}
	}

	return nil
}
