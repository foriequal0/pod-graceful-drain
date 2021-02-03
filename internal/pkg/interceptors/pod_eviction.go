package interceptors

import (
	"context"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"github.com/pkg/errors"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/api/policy/v1beta1"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
)

type PodEvictionInterceptor struct {
	drain     *core.PodGracefulDrain
	k8sClient client.Client
}

func NewPodEvictionInterceptor(drain *core.PodGracefulDrain, k8sClient client.Client) PodEvictionInterceptor {
	return PodEvictionInterceptor{
		drain:     drain,
		k8sClient: k8sClient,
	}
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=get;list;watch

func (i *PodEvictionInterceptor) Intercept(ctx context.Context, req *admission.Request, eviction *v1beta1.Eviction) (core.InterceptedAdmissionHandler, error) {
	if req.DryRun != nil && *req.DryRun == true {
		return core.DryRunHandler{}, nil
	}

	podKey := types.NamespacedName{
		Namespace: eviction.Namespace,
		Name:      eviction.Name,
	}
	pod := &corev1.Pod{}
	if err := i.k8sClient.Get(ctx, podKey, pod); err != nil {
		return nil, errors.Wrapf(err, "unable to get the pod")
	}

	interceptedHandler, err := i.drain.HandlePodRemove(ctx, pod)
	if err != nil {
		return nil, err
	}
	return interceptedHandler, nil
}
