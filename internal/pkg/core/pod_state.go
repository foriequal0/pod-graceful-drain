package core

import (
	"context"
	"github.com/pkg/errors"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/types"
	kubepod "k8s.io/kubernetes/pkg/api/v1/pod"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"time"
)

const (
	GracefulDrainPrefix         = "pod-graceful-drain"
	WaitLabelKey                = GracefulDrainPrefix + "/wait"
	DeleteAtAnnotationKey       = GracefulDrainPrefix + "/deleteAt"
	OriginalLabelsAnnotationKey = GracefulDrainPrefix + "/originalLabels"
)

func IsPodReady(pod *corev1.Pod) bool {
	if !kubepod.IsPodReady(pod) {
		return false
	}
	for _, rg := range pod.Spec.ReadinessGates {
		_, condition := kubepod.GetPodCondition(&pod.Status, rg.ConditionType)
		if condition == nil || condition.Status != corev1.ConditionTrue {
			return false
		}
	}
	return true
}

type PodDeletionDelayInfo struct {
	Isolated    bool
	Wait        bool
	DeleteAtUTC time.Time
}

func GetPodDeletionDelayInfo(pod *corev1.Pod) (PodDeletionDelayInfo, error) {
	result := PodDeletionDelayInfo{}

	waitLabelValue, hasWaitLabel := pod.Labels[WaitLabelKey]
	deleteAtAnnotationValue, hasDeleteAtLabel := pod.Annotations[DeleteAtAnnotationKey]

	result.Isolated = hasWaitLabel || hasDeleteAtLabel
	result.Wait = len(waitLabelValue) > 0

	if hasWaitLabel && !hasDeleteAtLabel {
		return result, errors.New("deleteAt annotation does not exits")
	}

	if !result.Wait {
		return result, nil
	}

	deleteAt, err := time.Parse(time.RFC3339, deleteAtAnnotationValue)
	if err != nil {
		return result, errors.Wrapf(err, "deleteAt annotation is not RFC3339 format")
	}
	result.DeleteAtUTC = deleteAt

	return result, nil
}

func (i *PodDeletionDelayInfo) GetRemainingTime(now time.Time) time.Duration {
	nowUTC := now.UTC()
	if !i.Isolated || !i.Wait || nowUTC.After(i.DeleteAtUTC) {
		return time.Duration(0)
	} else {
		return i.DeleteAtUTC.Sub(nowUTC)
	}
}

func IsPodInDrainingNode(ctx context.Context, client client.Client, pod *corev1.Pod) (bool, error) {
	nodeName := pod.Spec.NodeName
	var node corev1.Node
	if err := client.Get(ctx, types.NamespacedName{Name: nodeName}, &node); err != nil {
		return false, errors.Wrapf(err, "cannot get node %v", nodeName)
	}

	// Node is about to be drained.
	// `kubectl drain` will fail and stop if it meets the first pod that cannot be deleted.
	// It'll cordon a node before draining, so we detect it, and try not to deny the admission.
	if node.Spec.Unschedulable {
		return true, nil
	}
	for _, taint := range node.Spec.Taints {
		if taint.Key == corev1.TaintNodeUnschedulable {
			return true, nil
		}
	}
	return false, nil
}
