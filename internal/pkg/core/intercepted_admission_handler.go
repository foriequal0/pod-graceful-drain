package core

import (
	"fmt"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
	"time"
)

type InterceptedAdmissionHandler interface {
	HandleInterceptedAdmission() admission.Response
	String() string
}

var _ InterceptedAdmissionHandler = &DryRunHandler{}

type DryRunHandler struct{}

func (d DryRunHandler) HandleInterceptedAdmission() admission.Response {
	return admission.Allowed("")
}

func (d DryRunHandler) String() string {
	return fmt.Sprintf("dry-run")
}

var _ InterceptedAdmissionHandler = &DelayedNoDenyHandler{}

type DelayedNoDenyHandler struct {
	delayedTask DelayedTask
	duration    time.Duration
}

func NewDelayedNoDenyHandler(task DelayedTask, duration time.Duration) DelayedNoDenyHandler {
	return DelayedNoDenyHandler{
		delayedTask: task,
		duration:    duration,
	}
}

func (d DelayedNoDenyHandler) HandleInterceptedAdmission() admission.Response {
	err := d.delayedTask.RunAfterWait(d.duration)
	_ = err

	return admission.Allowed("")
}

func (d DelayedNoDenyHandler) String() string {
	return fmt.Sprintf("admission allow after for %v", d.duration.Truncate(time.Second).String())
}

var _ InterceptedAdmissionHandler = &AsyncWithDenyHandler{}

type AsyncWithDenyHandler struct {
	delayedTask DelayedTask
	duration    time.Duration
}

func NewAsyncWithDenyHandler(task DelayedTask, duration time.Duration) AsyncWithDenyHandler {
	return AsyncWithDenyHandler{
		delayedTask: task,
		duration:    duration,
	}
}

func (d AsyncWithDenyHandler) HandleInterceptedAdmission() admission.Response {
	if d.delayedTask != nil {
		d.delayedTask.RunAfterAsync(d.duration)
	}

	return admission.Denied("Pod cannot be removed immediately. It will be eventually removed after waiting for the load balancer to start.")
}

func (d AsyncWithDenyHandler) String() string {
	return fmt.Sprintf("admission deny, async delete after %v", d.duration.Truncate(time.Second).String())
}
