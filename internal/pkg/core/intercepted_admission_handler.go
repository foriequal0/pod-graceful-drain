package core

import (
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
	"time"
)

type InterceptedAdmissionHandler interface {
	HandleInterceptedAdmission() admission.Response
}

var _ InterceptedAdmissionHandler = &DryRunHandler{}

type DryRunHandler struct{}

func (d DryRunHandler) HandleInterceptedAdmission() admission.Response {
	return admission.Allowed("")
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

var _ InterceptedAdmissionHandler = &AsyncWithDenyHandler{}

type AsyncWithDenyHandler struct {
	delayedTask DelayedTask
	duration    time.Duration
}

func NewAsyncWithDenyHandler(task DelayedTask, duration time.Duration) DelayedNoDenyHandler {
	return DelayedNoDenyHandler{
		delayedTask: task,
		duration:    duration,
	}
}

func (d AsyncWithDenyHandler) HandleInterceptedAdmission() admission.Response {
	d.delayedTask.RunAfterAsync(d.duration)

	return admission.Denied("Pod cannot be removed immediately. It will be eventually removed after waiting for the load balancer to start interceptor.")
}
