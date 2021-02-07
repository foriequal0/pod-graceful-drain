package core

import (
	"context"
	"github.com/pkg/errors"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/api/policy/v1beta1"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
)

type Interceptor struct {
	drain     *PodGracefulDrain
	k8sClient client.Client
}

func NewInterceptor(drain *PodGracefulDrain, k8sClient client.Client) Interceptor {
	return Interceptor{
		drain:     drain,
		k8sClient: k8sClient,
	}
}

func (i *Interceptor) InterceptPodDeletion(ctx context.Context, req *admission.Request, pod *corev1.Pod) (InterceptedAdmissionResponse, error) {
	if req.DryRun != nil && *req.DryRun == true {
		return AdmissionResponse{Allow: true, Reason: "dry-run"}, nil
	}

	interceptedResponse, err := i.drain.DelayPodDeletion(ctx, pod)
	if err != nil {
		return nil, err
	}
	return interceptedResponse, nil
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=get;list;watch

func (i *Interceptor) InterceptPodEviction(ctx context.Context, req *admission.Request, eviction *v1beta1.Eviction) (InterceptedAdmissionResponse, error) {
	if req.DryRun != nil && *req.DryRun == true {
		return AdmissionResponse{Allow: true, Reason: "dry-run"}, nil
	}

	podKey := types.NamespacedName{
		Namespace: eviction.Namespace,
		Name:      eviction.Name,
	}
	pod := &corev1.Pod{}
	if err := i.k8sClient.Get(ctx, podKey, pod); err != nil {
		return nil, errors.Wrapf(err, "unable to get the pod")
	}

	interceptedResponse, err := i.drain.DelayPodDeletion(ctx, pod)
	if err != nil {
		return nil, err
	}
	return interceptedResponse, nil
}
