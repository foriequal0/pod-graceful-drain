package core

import (
	"context"
	"encoding/json"
	"github.com/pkg/errors"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/wait"
	"k8s.io/client-go/util/retry"
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

func Isolate(k8sClient client.Client, ctx context.Context, pod *corev1.Pod, deleteAt time.Time) error {
	patchCond := func(pod *corev1.Pod) bool {
		delayInfo, _ := GetPodDeletionDelayInfo(pod)
		return delayInfo.Isolated
	}
	patchMutate := func(pod *corev1.Pod) error {
		oldLabels, err := json.Marshal(pod.Labels)
		if err != nil {
			return err
		}

		pod.Labels = map[string]string{
			WaitLabelKey: "true",
		}
		if pod.Annotations == nil {
			pod.Annotations = map[string]string{}
		}
		pod.Annotations[DeleteAtAnnotationKey] = deleteAt.UTC().Format(time.RFC3339)
		pod.Annotations[OriginalLabelsAnnotationKey] = string(oldLabels)

		var newOwnerReferences []metav1.OwnerReference
		// To stop the GC kicking in, we cut the OwnerReferences.
		for _, item := range pod.OwnerReferences {
			newItem := item.DeepCopy()
			newItem.Controller = nil
			newOwnerReferences = append(newOwnerReferences, *newItem)
		}
		pod.OwnerReferences = newOwnerReferences

		return nil
	}

	return patchPod(k8sClient, ctx, pod, patchCond, patchMutate)
}

func DisableWaitLabel(k8sClient client.Client, ctx context.Context, pod *corev1.Pod) error {
	patchCond := func(pod *corev1.Pod) bool {
		existingLabel := pod.Labels[WaitLabelKey]
		return len(existingLabel) == 0
	}
	patchMutate := func(pod *corev1.Pod) error {
		// set empty rather than removing it. It helps to manually find delayed pods.
		pod.Labels[WaitLabelKey] = ""
		return nil
	}

	return patchPod(k8sClient, ctx, pod, patchCond, patchMutate)
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=patch;get

func patchPod(k8sClient client.Client, ctx context.Context, pod *corev1.Pod, desired func(*corev1.Pod) bool, mutate func(*corev1.Pod) error) error {
	needUpdate := false
	if len(pod.ResourceVersion) == 0 {
		needUpdate = true
	}

	for {
		if needUpdate {
			if err := getPod(k8sClient, ctx, pod); err != nil {
				return err
			}
		}

		if desired(pod) {
			return nil
		}

		oldPod := pod.DeepCopy()
		oldPod.UID = "" // only put the uid in the new object to ensure it appears in the patch as a precondition

		if err := mutate(pod); err != nil {
			return nil
		}

		podMergeOption := client.MergeFromWithOptions(oldPod, client.MergeFromWithOptimisticLock{})
		if err := k8sClient.Patch(ctx, pod, podMergeOption); err != nil {
			if apierrors.IsConflict(err) {
				needUpdate = false
				continue
			}
			return err
		}

		// see https://github.com/kubernetes-sigs/controller-runtime/issues/1257
		return wait.ExponentialBackoff(retry.DefaultBackoff, func() (bool, error) {
			if desired(pod) {
				return true, nil
			}
			err := getPod(k8sClient, ctx, pod)
			return false, err
		})
	}
}

func getPod(k8sClient client.Client, ctx context.Context, pod *corev1.Pod) error {
	podUID := pod.UID
	podKey := types.NamespacedName{
		Namespace: pod.Namespace,
		Name:      pod.Name,
	}

	var freshPod corev1.Pod
	if err := k8sClient.Get(ctx, podKey, &freshPod); err != nil {
		return err
	}
	if freshPod.UID != podUID {
		// UID conflict -> pod is gone
		return apierrors.NewNotFound(corev1.Resource(string(corev1.ResourcePods)), pod.Name)
	}

	*pod = freshPod
	return nil
}
