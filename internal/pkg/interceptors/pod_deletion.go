package interceptors

import (
	"context"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	corev1 "k8s.io/api/core/v1"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
)

type PodDeletionInterceptor struct {
	drain *core.PodGracefulDrain
}

func NewPodDeletionInterceptor(drain *core.PodGracefulDrain) PodDeletionInterceptor {
	return PodDeletionInterceptor{
		drain: drain,
	}
}

func (i *PodDeletionInterceptor) Intercept(ctx context.Context, req *admission.Request, pod *corev1.Pod) (core.InterceptedAdmissionHandler, error) {
	if req.DryRun != nil && *req.DryRun == true {
		return core.DryRunHandler{}, nil
	}

	interceptedHandler, err := i.drain.HandlePodRemove(ctx, pod)
	if err != nil {
		return nil, err
	}

	return interceptedHandler, nil
}
