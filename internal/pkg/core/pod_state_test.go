package core_test

import (
	"context"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"gotest.tools/assert"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"testing"
	"time"
)

func TestIsPodReady(t *testing.T) {
	tests := []struct {
		name  string
		given *corev1.Pod
		want  bool
	}{
		{
			name:  "pod is not ready if ready is not exist",
			given: &corev1.Pod{},
			want:  false,
		}, {
			name: "pod is not ready if PodReady is false",
			given: &corev1.Pod{
				Status: corev1.PodStatus{
					Conditions: []corev1.PodCondition{
						{
							Type:   corev1.PodReady,
							Status: corev1.ConditionFalse,
						},
					},
				},
			},
			want: false,
		}, {
			name: "pod is ready if PodReady is true",
			given: &corev1.Pod{
				Status: corev1.PodStatus{
					Conditions: []corev1.PodCondition{
						{
							Type:   corev1.PodReady,
							Status: corev1.ConditionTrue,
						},
					},
				},
			},
			want: true,
		}, {
			name: "pod is not ready if ReadinessGate condition is not exists",
			given: &corev1.Pod{
				Spec: corev1.PodSpec{
					ReadinessGates: []corev1.PodReadinessGate{
						{ConditionType: "readiness-gate"},
					},
				},
				Status: corev1.PodStatus{
					Conditions: []corev1.PodCondition{
						{
							Type:   corev1.PodReady,
							Status: corev1.ConditionTrue,
						},
					},
				},
			},
			want: false,
		}, {
			name: "pod is not ready if ReadinessGate condition is false",
			given: &corev1.Pod{
				Spec: corev1.PodSpec{
					ReadinessGates: []corev1.PodReadinessGate{
						{ConditionType: "readiness-gate"},
					},
				},
				Status: corev1.PodStatus{
					Conditions: []corev1.PodCondition{
						{
							Type:   corev1.PodReady,
							Status: corev1.ConditionTrue,
						},
						{
							Type:   corev1.PodConditionType("readiness-gate"),
							Status: corev1.ConditionFalse,
						},
					},
				},
			},
			want: false,
		}, {
			name: "pod is not ready if ReadinessGate condition is false",
			given: &corev1.Pod{
				Spec: corev1.PodSpec{
					ReadinessGates: []corev1.PodReadinessGate{
						{ConditionType: "readiness-gate"},
					},
				},
				Status: corev1.PodStatus{
					Conditions: []corev1.PodCondition{
						{
							Type:   corev1.PodReady,
							Status: corev1.ConditionTrue,
						},
						{
							Type:   corev1.PodConditionType("readiness-gate"),
							Status: corev1.ConditionTrue,
						},
					},
				},
			},
			want: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := core.IsPodReady(tt.given)
			assert.Equal(t, result, tt.want)
		})
	}
}

func TestGetPodDeletionDelayInfo(t *testing.T) {
	deleteAt := time.Now().UTC().Truncate(time.Second)
	correctDeleteAtLabel := deleteAt.Format(time.RFC3339)
	incorrectDeleteAtLabel := deleteAt.Format(time.ANSIC)

	tests := []struct {
		name    string
		given   *corev1.Pod
		want    core.PodDeletionDelayInfo
		errwant string
	}{
		{
			name:  "plain pod",
			given: &corev1.Pod{},
			want: core.PodDeletionDelayInfo{
				Isolated: false,
				Wait:     false,
			},
		}, {
			name: "pod has non-empty wait sentinel label and deleteAt annotation",
			given: &corev1.Pod{
				ObjectMeta: metav1.ObjectMeta{
					Labels: map[string]string{
						"pod-graceful-drain/wait": "true",
					},
					Annotations: map[string]string{
						"pod-graceful-drain/deleteAt": correctDeleteAtLabel,
					},
				},
			},
			want: core.PodDeletionDelayInfo{
				Isolated:    true,
				Wait:        true,
				DeleteAtUTC: deleteAt,
			},
		}, {
			name: "pod has empty wait sentinel label",
			given: &corev1.Pod{
				ObjectMeta: metav1.ObjectMeta{
					Labels: map[string]string{
						"pod-graceful-drain/wait": "",
					},
					Annotations: map[string]string{
						"pod-graceful-drain/deleteAt": correctDeleteAtLabel,
					},
				},
			},
			want: core.PodDeletionDelayInfo{
				Isolated: true,
				Wait:     false,
			},
		}, {
			name: "pod only has deleteAt annotation",
			given: &corev1.Pod{
				ObjectMeta: metav1.ObjectMeta{
					Annotations: map[string]string{
						"pod-graceful-drain/deleteAt": correctDeleteAtLabel,
					},
				},
			},
			want: core.PodDeletionDelayInfo{
				Isolated: true,
				Wait:     false,
			},
		}, {
			name: "pod doesn't have deleteAt label",
			given: &corev1.Pod{
				ObjectMeta: metav1.ObjectMeta{
					Labels: map[string]string{
						"pod-graceful-drain/wait": "true",
					},
				},
			},
			errwant: "deleteAt annotation does not exits",
		}, {
			name: "pod has incorrect deleteAt label",
			given: &corev1.Pod{
				ObjectMeta: metav1.ObjectMeta{
					Labels: map[string]string{
						"pod-graceful-drain/wait": "true",
					},
					Annotations: map[string]string{
						"pod-graceful-drain/deleteAt": incorrectDeleteAtLabel,
					},
				},
			},
			errwant: "deleteAt annotation is not RFC3339 format",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := core.GetPodDeletionDelayInfo(tt.given)
			if err != nil {
				assert.ErrorContains(t, err, tt.errwant)
			} else {
				assert.Equal(t, result, tt.want)
			}
		})
	}
}

func TestPodDeletionDelayInfo_GetRemainingTime(t *testing.T) {
	deleteAt := time.Now().UTC().Truncate(time.Second)
	offset := 30 * time.Second
	delayInfo := core.PodDeletionDelayInfo{
		Isolated:    true,
		Wait:        true,
		DeleteAtUTC: deleteAt,
	}

	tests := []struct {
		name string
		now  time.Time
		want time.Duration
	}{
		{
			name: "before deleteAt",
			now:  deleteAt.Add(-offset),
			want: offset,
		}, {
			name: "after deleteAt",
			now:  deleteAt.Add(offset),
			want: time.Duration(0),
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := delayInfo.GetRemainingTime(tt.now)
			assert.Equal(t, result, tt.want)
		})
	}
}

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
		existing []*corev1.Pod
		given    *corev1.Pod
		want     *corev1.Pod
	}{
		{
			name:     "pod should be isolated by attaching wait sentinel label, must have deleteAt annotation",
			existing: []*corev1.Pod{normalPod},
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
			existing: []*corev1.Pod{isolatedPod1},
			given:    normalPod,
			want:     isolatedPod1,
		}, {
			name:     "already isolated pod shouldn't be modified (2)",
			existing: []*corev1.Pod{isolatedPod2},
			given:    normalPod,
			want:     isolatedPod2,
		}, {
			name:     "already isolated pod shouldn't be modified (3)",
			existing: []*corev1.Pod{isolatedPod3},
			given:    normalPod,
			want:     isolatedPod3,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx := context.Background()
			k8sSchema := runtime.NewScheme()
			assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
			k8sClient := fake.NewFakeClientWithScheme(k8sSchema)
			for _, existing := range tt.existing {
				assert.NilError(t, k8sClient.Create(ctx, existing.DeepCopy()))
			}

			pod := tt.given.DeepCopy()
			err := core.Isolate(k8sClient, ctx, pod, deleteAt)

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
		existing []*corev1.Pod
		given    *corev1.Pod
		want     *corev1.Pod
	}{
		{
			name:     "waiting pod should be disabled by setting empty string on the wait sentinel label",
			existing: []*corev1.Pod{waitingPod},
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
			existing: []*corev1.Pod{disabledPod1},
			given:    waitingPod,
			want:     disabledPod1,
		}, {
			name:     "already disabled pod shouldn't be modified (2)",
			existing: []*corev1.Pod{disabledPod2},
			given:    waitingPod,
			want:     disabledPod2,
		}, {
			name:     "already disabled pod shouldn't be modified (3)",
			existing: []*corev1.Pod{disabledPod3},
			given:    waitingPod,
			want:     disabledPod3,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx := context.Background()
			k8sSchema := runtime.NewScheme()
			assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
			k8sClient := fake.NewFakeClientWithScheme(k8sSchema)
			for _, existing := range tt.existing {
				assert.NilError(t, k8sClient.Create(ctx, existing.DeepCopy()))
			}

			pod := tt.given.DeepCopy()
			err := core.DisableWaitLabel(k8sClient, ctx, pod)

			assert.NilError(t, err)
			assert.DeepEqual(t, pod.Labels, tt.want.Labels)
		})
	}
}
