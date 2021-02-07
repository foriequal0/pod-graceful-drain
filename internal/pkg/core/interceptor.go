package core

import (
	"context"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/api/policy/v1beta1"
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

func (i *Interceptor) InterceptPodEviction(ctx context.Context, req *admission.Request, eviction *v1beta1.Eviction) (InterceptedAdmissionResponse, error) {
	if req.DryRun != nil && *req.DryRun == true {
		return AdmissionResponse{Allow: true, Reason: "dry-run"}, nil
	}

	intercepted, err := i.drain.DelayPodEviction(ctx, eviction)
	if err != nil || !intercepted {
		return nil, err
	}

	response, err := NewEvictionResponse(eviction)
	if err != nil {
		return nil, err
	}
	return response, nil
}
