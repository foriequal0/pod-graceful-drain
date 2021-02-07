package core

import (
	"context"
	"github.com/go-logr/logr"
	"github.com/pkg/errors"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/api/policy/v1beta1"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/manager"
	"time"
)

const (
	fallbackAdmissionDelayTimeout         = 30 * time.Second
	admissionDelayOverhead                = 2 * time.Second
	defaultPodGracefulDrainCleanupTimeout = 10 * time.Second
)

type PodGracefulDrain struct {
	client  client.Client
	logger  logr.Logger
	config  *PodGracefulDrainConfig
	delayer Delayer
}

var _ manager.Runnable = &PodGracefulDrain{}

func NewPodGracefulDrain(k8sClient client.Client, logger logr.Logger, config *PodGracefulDrainConfig) PodGracefulDrain {
	return PodGracefulDrain{
		client:  k8sClient,
		logger:  logger.WithName("pod-graceful-drain"),
		config:  config,
		delayer: NewDelayer(logger),
	}
}

func (d *PodGracefulDrain) DelayPodDeletion(ctx context.Context, pod *corev1.Pod) (InterceptedAdmissionResponse, error) {
	now := time.Now()
	logger := d.getLoggerFor(pod)
	spec, err := d.getDelayedPodDeletionSpec(ctx, pod, now)
	if err != nil || spec == nil {
		return nil, err
	}

	spec.log(logger)

	if err := spec.execute(ctx, NewPodMutator(d.client, pod).WithLogger(logger)); err != nil {
		return nil, err
	}
	return spec.admission, nil
}

type delayedPodDeletionSpec struct {
	isolate         bool
	deleteAt        time.Time
	asyncDeleteTask DelayedTask
	sleepTask       DelayedTask
	reason          string
	admission       AdmissionResponse
}

func (d *PodGracefulDrain) getDelayedPodDeletionSpec(ctx context.Context, pod *corev1.Pod, now time.Time) (spec *delayedPodDeletionSpec, err error) {
	if !IsPodReady(pod) {
		return nil, nil
	}

	delayInfo, err := GetPodDeletionDelayInfo(pod)
	if err != nil {
		return nil, errors.Wrapf(err, "unable to get pod deletion info")
	} else if delayInfo.Isolated {
		spec, err := d.getReentrySpec(ctx, pod, delayInfo, now)
		if err != nil {
			return nil, errors.Wrapf(err, "unable to getPodDelayedRemoveSpec pod deletion reentry")
		}
		return spec, nil
	}

	hadServiceTargetTypeIP, err := DidPodHaveServicesTargetTypeIP(ctx, d.client, pod)
	if err != nil {
		return nil, errors.Wrapf(err, "unable to determine whether the pod had service with ip target-type")
	} else if !hadServiceTargetTypeIP {
		return nil, nil
	}

	canDeny, reason, err := d.canDenyAdmission(ctx, pod)
	if err != nil {
		return nil, errors.Wrap(err, "unable to determine whether it can be denied")
	} else if canDeny {
		spec = &delayedPodDeletionSpec{
			isolate:         true,
			deleteAt:        now.Add(d.config.DeleteAfter),
			asyncDeleteTask: d.getDelayedPodDeletionTask(pod, d.config.DeleteAfter),
			reason:          reason,
			admission: AdmissionResponse{
				Allow:  false,
				Reason: "Pod cannot be removed immediately. It will be eventually removed after waiting for the load balancer to start",
			},
		}
	} else {
		deleteAfter := getAdmissionDelayTimeout(ctx, now)
		spec = &delayedPodDeletionSpec{
			isolate:   true,
			deleteAt:  now.Add(deleteAfter),
			sleepTask: d.getSleepTask(deleteAfter),
			reason:    reason,
			admission: AdmissionResponse{
				Allow:  true,
				Reason: "Pod deletion is delayed enough",
			},
		}
	}
	return
}

func getAdmissionDelayTimeout(ctx context.Context, now time.Time) time.Duration {
	timeout := fallbackAdmissionDelayTimeout
	if deadline, ok := ctx.Deadline(); ok {
		timeout = deadline.Sub(now) - admissionDelayOverhead
		if timeout < 0 {
			timeout = time.Duration(0)
		}
	}
	return timeout
}

func (s *delayedPodDeletionSpec) log(logger logr.Logger) {
	details := map[string]interface{}{}
	if s.isolate {
		details["isolate"] = map[string]interface{}{
			"deleteAt": s.deleteAt,
		}
	}
	if s.asyncDeleteTask != nil {
		details["asyncDelete"] = map[string]interface{}{
			"taskId":   s.asyncDeleteTask.GetId(),
			"duration": s.asyncDeleteTask.GetDuration(),
		}
	}
	if s.sleepTask != nil {
		details["sleep"] = map[string]interface{}{
			"taskId":   s.sleepTask.GetId(),
			"duration": s.sleepTask.GetDuration(),
		}
	}

	logger.Info("delayed pod remove spec",
		"details", details,
		"reason", s.reason,
		"admission", s.admission.Allow)
}

func (s *delayedPodDeletionSpec) execute(ctx context.Context, m *PodMutator) error {
	if s.isolate {
		if err := m.Isolate(ctx, s.deleteAt); err != nil {
			return errors.Wrap(err, "unable to isolate the pod")
		}
	}

	if s.asyncDeleteTask != nil {
		s.asyncDeleteTask.RunAsync()
	}

	if s.sleepTask != nil {
		if err := s.sleepTask.RunWait(ctx); err != nil {
			return err
		}
	}
	return nil
}

// getReentrySpec handles these cases:
// * apiserver immediately retried the deletion when we patched the pod and denied the admission
//   since it is indistinguishable from the collision. So it should keep deny.
// * We disabled wait sentinel label and deleted the pod, but the patch hasn't been propagated fast enough
//   so ValidatingAdmissionWebhook read the wait label of the old version
//   => deletePodAfter will retry with back-offs, so we keep denying the admission.
// * Users and controllers manually tries to delete the pod before deleteAt.
//   => User can see the admission report message. Controller should getDelayedPodDeletionSpec admission failures.
func (d *PodGracefulDrain) getReentrySpec(ctx context.Context, pod *corev1.Pod, info PodDeletionDelayInfo, now time.Time) (spec *delayedPodDeletionSpec, err error) {
	remainingTime := info.GetRemainingTime(now)
	if remainingTime == time.Duration(0) {
		return nil, nil
	}

	canDeny, reason, err := d.canDenyAdmission(ctx, pod)
	if err != nil {
		return nil, errors.Wrap(err, "cannot determine whether it should be denied")
	} else if canDeny {
		spec = &delayedPodDeletionSpec{
			reason: reason,
			admission: AdmissionResponse{
				Allow:  false,
				Reason: "Pod cannot be removed immediately. It will be eventually removed after waiting for the load balancer to start (reentry)",
			},
		}
	} else {
		timeout := getAdmissionDelayTimeout(ctx, now)
		if remainingTime > timeout {
			remainingTime = timeout
		}
		// All admissions should be delayed. Pods will be deleted if any of admissions is finished.
		spec = &delayedPodDeletionSpec{
			sleepTask: d.getSleepTask(remainingTime),
			reason:    reason,
			admission: AdmissionResponse{
				Allow:  true,
				Reason: "Pod deletion is delayed enough (reentry)",
			},
		}
	}
	return
}

// +kubebuilder:rbac:groups="",resources=nodes,verbs=get;list;watch

func (d *PodGracefulDrain) canDenyAdmission(ctx context.Context, pod *corev1.Pod) (bool, string, error) {
	if d.config.NoDenyAdmission {
		return false, "no-deny-admission config", nil
	}

	draining, err := IsPodInDrainingNode(ctx, d.client, pod)
	if err != nil {
		return false, "", nil
	} else if draining {
		return false, "node might be draining", nil
	}
	return true, "default", nil
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=get;list;watch

func (d *PodGracefulDrain) DelayPodEviction(ctx context.Context, eviction *v1beta1.Eviction) (bool, error) {
	now := time.Now()
	logger := d.getLoggerFor(eviction)

	podKey := types.NamespacedName{
		Namespace: eviction.Namespace,
		Name:      eviction.Name,
	}
	pod := &corev1.Pod{}
	if err := d.client.Get(ctx, podKey, pod); err != nil {
		return false, errors.Wrapf(err, "unable to get the pod")
	}

	spec, err := d.getDelayedPodEvictionSpec(ctx, pod, now)
	if err != nil || spec == nil {
		return false, err
	}

	spec.log(logger)

	if err := spec.execute(ctx, NewPodMutator(d.client, pod).WithLogger(logger)); err != nil {
		return false, err
	}

	return true, nil
}

type delayedPodEvictionSpec struct {
	isolate         bool
	deleteAt        time.Time
	asyncDeleteTask DelayedTask
}

func (d *PodGracefulDrain) getDelayedPodEvictionSpec(ctx context.Context, pod *corev1.Pod, now time.Time) (spec *delayedPodEvictionSpec, err error) {
	if !IsPodReady(pod) {
		return nil, nil
	}

	delayInfo, err := GetPodDeletionDelayInfo(pod)
	if err != nil {
		return nil, errors.Wrapf(err, "unable to get pod deletion info")
	} else if delayInfo.Isolated {
		remainingTime := delayInfo.GetRemainingTime(now)
		if remainingTime == time.Duration(0) {
			return nil, nil
		}

		// reentry
		return &delayedPodEvictionSpec{}, nil
	}

	hadServiceTargetTypeIP, err := DidPodHaveServicesTargetTypeIP(ctx, d.client, pod)
	if err != nil {
		return nil, errors.Wrapf(err, "unable to determine whether the pod had service with ip target-type")
	} else if !hadServiceTargetTypeIP {
		return nil, nil
	}

	spec = &delayedPodEvictionSpec{
		isolate:         true,
		deleteAt:        now.Add(d.config.DeleteAfter),
		asyncDeleteTask: d.getDelayedPodDeletionTask(pod, d.config.DeleteAfter),
	}
	return
}

func (s *delayedPodEvictionSpec) log(logger logr.Logger) {
	details := map[string]interface{}{}
	if s.isolate {
		details["isolate"] = map[string]interface{}{
			"deleteAt": s.deleteAt,
		}
	}
	if s.asyncDeleteTask != nil {
		details["asyncDelete"] = map[string]interface{}{
			"taskId":   s.asyncDeleteTask.GetId(),
			"duration": s.asyncDeleteTask.GetDuration(),
		}
	}

	logger.Info("delayed pod eviction spec",
		"details", details)
}

func (s *delayedPodEvictionSpec) execute(ctx context.Context, m *PodMutator) error {
	if s.isolate {
		if err := m.Isolate(ctx, s.deleteAt); err != nil {
			return errors.Wrap(err, "unable to isolate the pod")
		}
	}

	if s.asyncDeleteTask != nil {
		s.asyncDeleteTask.RunAsync()
	}

	return nil
}

func (d *PodGracefulDrain) Start(ctx context.Context) error {
	d.logger.Info("starting pod-graceful-drain")
	if err := d.cleanupPreviousRun(ctx); err != nil {
		d.logger.Error(err, "error while cleaning pods up that are not removed in the previous run")
	}

	<-ctx.Done()

	d.logger.Info("stopping pod-graceful-drain")

	drainTimeout := fallbackAdmissionDelayTimeout
	if drainTimeout < d.config.DeleteAfter {
		drainTimeout = d.config.DeleteAfter
	}

	d.delayer.Stop(drainTimeout, defaultPodGracefulDrainCleanupTimeout)
	d.logger.V(1).Info("stopped pod-graceful-drain")
	return nil
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=list;watch

func (d *PodGracefulDrain) cleanupPreviousRun(ctx context.Context) error {
	podList := &corev1.PodList{}
	// select all pods regardless of its value. These pods were about to be deleted anyway when its value is empty.
	if err := d.client.List(ctx, podList, client.HasLabels{WaitLabelKey}); err != nil {
		return errors.Wrapf(err, "cannot list pods with wait sentinel label")
	}

	now := time.Now()
	for idx := range podList.Items {
		pod := &podList.Items[idx]

		deleteAfter := d.config.DeleteAfter
		delayInfo, err := GetPodDeletionDelayInfo(pod)
		if err != nil {
			d.getLoggerFor(pod).Error(err, "cannot get pod deletion delay info, but it has wait sentinel label")
		} else {
			deleteAfter = delayInfo.GetRemainingTime(now)
		}

		d.getDelayedPodDeletionTask(pod, deleteAfter).RunAsync()
	}
	return nil
}

func (d *PodGracefulDrain) getLoggerFor(obj client.Object) logr.Logger {
	namespacedName := types.NamespacedName{
		Namespace: obj.GetNamespace(),
		Name:      obj.GetName(),
	}

	return d.logger.WithValues(obj.GetObjectKind().GroupVersionKind().Kind, namespacedName.String())
}

func (d *PodGracefulDrain) getDelayedPodDeletionTask(pod *corev1.Pod, duration time.Duration) DelayedTask {
	return d.delayer.NewTask(duration, func(ctx context.Context, _ bool) error {
		return NewPodMutator(d.client, pod).
			WithLogger(logr.FromContextOrDiscard(ctx)).
			DisableWaitLabelAndDelete(ctx)
	})
}

func (d *PodGracefulDrain) getSleepTask(duration time.Duration) DelayedTask {
	return d.delayer.NewTask(duration, nil)
}
