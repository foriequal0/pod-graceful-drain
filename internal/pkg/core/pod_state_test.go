package core_test

import (
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"gotest.tools/assert"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
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
