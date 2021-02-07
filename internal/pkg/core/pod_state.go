package core

import (
	"github.com/pkg/errors"
	corev1 "k8s.io/api/core/v1"
	kubepod "k8s.io/kubernetes/pkg/api/v1/pod"
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
