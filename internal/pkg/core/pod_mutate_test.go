package core

import (
	"context"
	"gotest.tools/assert"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"testing"
	"time"
)

func TestIsolate(t *testing.T) {
	deleteAt := time.Now().UTC().Truncate(time.Second)
	deleteAtLabel := deleteAt.Format(time.RFC3339)
	normalPod := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"label1": "value1",
			},
		},
	}
	isolatedPod1 := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"pod-graceful-drain/wait": "true",
			},
			Annotations: map[string]string{
				"pod-graceful-drain/deleteAt": deleteAtLabel,
			},
		},
	}
	isolatedPod2 := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"pod-graceful-drain/wait": "",
			},
			Annotations: map[string]string{
				"pod-graceful-drain/deleteAt": deleteAtLabel,
			},
		},
	}
	isolatedPod3 := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Annotations: map[string]string{
				"pod-graceful-drain/deleteAt": deleteAtLabel,
			},
		},
	}

	tests := []struct {
		name     string
		existing []runtime.Object
		given    *corev1.Pod
		want     *corev1.Pod
	}{
		{
			name:     "pod should be isolated by attaching wait sentinel label, must have deleteAt annotation",
			existing: []runtime.Object{normalPod},
			given:    normalPod,
			want: &corev1.Pod{
				ObjectMeta: metav1.ObjectMeta{
					Name: "pod",
					Labels: map[string]string{
						"pod-graceful-drain/wait": "true",
					},
					Annotations: map[string]string{
						"pod-graceful-drain/deleteAt":       deleteAtLabel,
						"pod-graceful-drain/originalLabels": `{"label1":"value1"}`,
					},
				},
			},
		}, {
			name:     "already isolated pod shouldn't be modified (1)",
			existing: []runtime.Object{isolatedPod1},
			given:    normalPod,
			want:     isolatedPod1,
		}, {
			name:     "already isolated pod shouldn't be modified (2)",
			existing: []runtime.Object{isolatedPod2},
			given:    normalPod,
			want:     isolatedPod2,
		}, {
			name:     "already isolated pod shouldn't be modified (3)",
			existing: []runtime.Object{isolatedPod3},
			given:    normalPod,
			want:     isolatedPod3,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx := context.Background()
			k8sSchema := runtime.NewScheme()
			assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
			builder := fake.NewClientBuilder().WithScheme(k8sSchema)
			for _, existing := range tt.existing {
				builder = builder.WithRuntimeObjects(existing.DeepCopyObject())
			}
			k8sClient := builder.Build()

			pod := tt.given.DeepCopy()
			err := NewPodMutator(k8sClient, pod).isolate(ctx, deleteAt)

			assert.NilError(t, err)
			assert.DeepEqual(t, pod.Labels, tt.want.Labels)
			assert.DeepEqual(t, pod.Annotations, tt.want.Annotations)
		})
	}
}

func TestDisableWaitLabel(t *testing.T) {
	waitingPod := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"pod-graceful-drain/wait": "true",
			},
		},
	}
	disabledPod1 := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"pod-graceful-drain/wait": "",
			},
		},
	}
	disabledPod2 := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
		},
	}
	disabledPod3 := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"label1": "value1",
			},
		},
	}

	tests := []struct {
		name     string
		existing []runtime.Object
		given    *corev1.Pod
		want     *corev1.Pod
	}{
		{
			name:     "waiting pod should be disabled by setting empty string on the wait sentinel label",
			existing: []runtime.Object{waitingPod},
			given:    waitingPod,
			want: &corev1.Pod{
				ObjectMeta: metav1.ObjectMeta{
					Name: "pod",
					Labels: map[string]string{
						"pod-graceful-drain/wait": "",
					},
				},
			},
		}, {
			name:     "already disabled pod shouldn't be modified (1)",
			existing: []runtime.Object{disabledPod1},
			given:    waitingPod,
			want:     disabledPod1,
		}, {
			name:     "already disabled pod shouldn't be modified (2)",
			existing: []runtime.Object{disabledPod2},
			given:    waitingPod,
			want:     disabledPod2,
		}, {
			name:     "already disabled pod shouldn't be modified (3)",
			existing: []runtime.Object{disabledPod3},
			given:    waitingPod,
			want:     disabledPod3,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx := context.Background()
			k8sSchema := runtime.NewScheme()
			assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
			builder := fake.NewClientBuilder().WithScheme(k8sSchema)
			for _, existing := range tt.existing {
				builder = builder.WithRuntimeObjects(existing.DeepCopyObject())
			}
			k8sClient := builder.Build()

			pod := tt.given.DeepCopy()
			err := NewPodMutator(k8sClient, pod).disableWaitLabel(ctx)

			assert.NilError(t, err)
			assert.DeepEqual(t, pod.Labels, tt.want.Labels)
		})
	}
}

func TestDelete(t *testing.T) {
	pod := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"pod-graceful-drain/wait": "true",
			},
		},
	}

	tests := []struct {
		name     string
		existing []runtime.Object
		given    *corev1.Pod
	}{
		{
			name:     "delete",
			existing: []runtime.Object{pod},
			given:    pod,
		},
		{
			name:     "delete gone",
			existing: []runtime.Object{},
			given:    pod,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx := context.Background()
			k8sSchema := runtime.NewScheme()
			assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
			builder := fake.NewClientBuilder().WithScheme(k8sSchema)
			for _, existing := range tt.existing {
				builder = builder.WithRuntimeObjects(existing.DeepCopyObject())
			}
			k8sClient := builder.Build()

			pod := tt.given.DeepCopy()
			err := NewPodMutator(k8sClient, pod).delete(ctx)

			assert.NilError(t, err)
		})
	}
}
