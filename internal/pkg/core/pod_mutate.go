package core

import (
	"context"
	"encoding/json"
	"github.com/go-logr/logr"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/wait"
	"k8s.io/client-go/util/retry"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"time"
)

type PodMutator struct {
	client client.Client
	logger logr.Logger
	pod    *corev1.Pod
}

func NewPodMutator(client client.Client, pod *corev1.Pod) *PodMutator {
	return &PodMutator{
		client: client,
		logger: logr.Discard(),
		pod:    pod,
	}
}

func (m *PodMutator) WithLogger(logger logr.Logger) *PodMutator {
	return &PodMutator{
		client: m.client,
		logger: logger.WithValues("pod", types.NamespacedName{
			Namespace: m.pod.Namespace,
			Name:      m.pod.Name,
		}),
		pod: m.pod,
	}
}

func (m *PodMutator) Isolate(ctx context.Context, deleteAt time.Time) error {
	m.logger.Info("isolating")
	if err := m.isolate(ctx, deleteAt); err != nil {
		return err
	}
	m.logger.V(1).Info("isolated")
	return nil
}

func (m *PodMutator) DisableWaitLabelAndDelete(ctx context.Context) error {
	m.logger.Info("disabling wait label")
	if err := m.disableWaitLabel(ctx); err != nil {
		return err
	}
	m.logger.Info("disabled wait label")

	m.logger.Info("deleting")
	if err := m.delete(ctx); err != nil {
		return err
	}
	m.logger.V(1).Info("deleted")
	return nil
}

func (m *PodMutator) isolate(ctx context.Context, deleteAt time.Time) error {
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

	return m.patchPod(ctx, patchCond, patchMutate)
}

func (m *PodMutator) disableWaitLabel(ctx context.Context) error {
	patchCond := func(pod *corev1.Pod) bool {
		existingLabel := pod.Labels[WaitLabelKey]
		return len(existingLabel) == 0
	}
	patchMutate := func(pod *corev1.Pod) error {
		// set empty rather than removing it. It helps to manually find delayed pods.
		pod.Labels[WaitLabelKey] = ""
		return nil
	}

	return m.patchPod(ctx, patchCond, patchMutate)
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=patch

func (m *PodMutator) patchPod(ctx context.Context, desired func(*corev1.Pod) bool, mutate func(*corev1.Pod) error) error {
	needUpdate := false
	if len(m.pod.ResourceVersion) == 0 {
		needUpdate = true
	}

	for {
		if needUpdate {
			if err := m.reloadPod(ctx); err != nil {
				return err
			}
		}

		if desired(m.pod) {
			return nil
		}

		oldPod := m.pod.DeepCopy()
		oldPod.UID = "" // only put the uid in the new object to ensure it appears in the patch as a precondition

		if err := mutate(m.pod); err != nil {
			return nil
		}

		podMergeOption := client.MergeFromWithOptions(oldPod, client.MergeFromWithOptimisticLock{})
		if err := m.client.Patch(ctx, m.pod, podMergeOption); err != nil {
			if apierrors.IsConflict(err) {
				needUpdate = false
				continue
			}
			return err
		}

		// see https://github.com/kubernetes-sigs/controller-runtime/issues/1257
		return wait.ExponentialBackoff(retry.DefaultBackoff, func() (bool, error) {
			if desired(m.pod) {
				return true, nil
			}
			err := m.reloadPod(ctx)
			return false, err
		})
	}
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=get;watch

func (m *PodMutator) reloadPod(ctx context.Context) error {
	podUID := m.pod.UID
	podKey := types.NamespacedName{
		Namespace: m.pod.Namespace,
		Name:      m.pod.Name,
	}

	var freshPod corev1.Pod
	if err := m.client.Get(ctx, podKey, &freshPod); err != nil {
		return err
	}
	if freshPod.UID != podUID {
		// UID conflict -> pod is gone
		return apierrors.NewNotFound(corev1.Resource(string(corev1.ResourcePods)), m.pod.Name)
	}

	*m.pod = freshPod
	return nil
}

// +kubebuilder:rbac:groups="",resources=pods,verbs=delete

func (m *PodMutator) delete(ctx context.Context) error {
	return wait.ExponentialBackoff(retry.DefaultBackoff, func() (bool, error) {
		if err := m.client.Delete(ctx, m.pod, client.Preconditions{UID: &m.pod.UID}); err != nil {
			if apierrors.IsNotFound(err) || apierrors.IsConflict(err) {
				// The pod is already deleted. Okay to ignore
				return true, nil
			}
			// Intercept might deny the deletion as too early until DisableWaitLabel patch is propagated.
			// TODO: error is actually admission denial
			return false, nil
		}
		return true, nil
	})
}
