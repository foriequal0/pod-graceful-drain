package core

import (
	"context"
	"github.com/pkg/errors"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/labels"
	"k8s.io/apimachinery/pkg/types"
	elbv2api "sigs.k8s.io/aws-load-balancer-controller/apis/elbv2/v1beta1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"strings"
)

const (
	// Prefix for TargetHealth pod condition type.
	TargetHealthPodConditionTypePrefix = "target-health.elbv2.k8s.aws"
)

// +kubebuilder:rbac:groups=elbv2.k8s.aws,resources=targetgroupbindings,verbs=list;watch
// +kubebuilder:rbac:groups="",resources=services,verbs=get;list;watch

func DidPodHaveServicesTargetTypeIP(ctx context.Context, client client.Client, pod *corev1.Pod) (bool, error) {
	svcs, err := getServicesTargetTypeIP(client, ctx, pod)
	if err != nil {
		return false, err
	}

	if len(svcs) == 0 {
		for _, item := range pod.Spec.ReadinessGates {
			if strings.HasPrefix(string(item.ConditionType), TargetHealthPodConditionTypePrefix) {
				// The pod once had TargetGroupBindings, but it is somehow gone.
				// We don't know whether its TargetType is IP, it's target group, etc.
				// It might be worth to to give some time to ELB.
				return true, nil
			}
		}
		return false, nil
	}
	return true, nil
}

func getServicesTargetTypeIP(k8sClient client.Client, ctx context.Context, pod *corev1.Pod) ([]corev1.Service, error) {
	tgbList := &elbv2api.TargetGroupBindingList{}
	if err := k8sClient.List(ctx, tgbList, client.InNamespace(pod.Namespace)); err != nil {
		return nil, errors.Wrapf(err, "unable to list TargetGroupBindings in namespace %v", pod.Namespace)
	}
	var svcs []corev1.Service
	for _, tgb := range tgbList.Items {
		if tgb.Spec.TargetType == nil || (*tgb.Spec.TargetType) != elbv2api.TargetTypeIP {
			continue
		}

		svcKey := types.NamespacedName{Namespace: tgb.Namespace, Name: tgb.Spec.ServiceRef.Name}
		svc := corev1.Service{}
		if err := k8sClient.Get(ctx, svcKey, &svc); err != nil {
			// If the service is not found, ignore
			if apierrors.IsNotFound(err) {
				continue
			}
			return nil, err
		}
		var svcSelector labels.Selector
		if len(svc.Spec.Selector) == 0 {
			svcSelector = labels.Nothing()
		} else {
			svcSelector = labels.SelectorFromSet(svc.Spec.Selector)
		}
		if svcSelector.Matches(labels.Set(pod.Labels)) {
			svcs = append(svcs, svc)
		}
	}
	return svcs, nil
}
