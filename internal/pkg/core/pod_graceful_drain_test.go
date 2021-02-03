package core

import (
	"context"
	"fmt"
	"gotest.tools/assert"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	elbv2 "sigs.k8s.io/aws-load-balancer-controller/apis/elbv2/v1beta1"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
	"testing"
	"time"
)

const (
	deleteAfter    = 90 * time.Second
	admissionDelay = 25 * time.Second
)

var (
	defaultConfig = PodGracefulDrainConfig{
		DeleteAfter:     deleteAfter,
		NoDenyAdmission: false,
		AdmissionDelay:  admissionDelay,
	}
	noDenyConfig = PodGracefulDrainConfig{
		NoDenyAdmission: true,
		AdmissionDelay:  admissionDelay,
	}
)

// tgb, readinessgate, node status
//

func TestPodDelayedRemoveSpec(t *testing.T) {
	now := time.Now().Truncate(time.Second)
	deleteAt := now.UTC().Add(deleteAfter).Format(time.RFC3339)

	readyStatus := corev1.PodStatus{
		Conditions: []corev1.PodCondition{
			{Type: corev1.PodReady, Status: corev1.ConditionTrue},
		},
	}
	boundPod := corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"selector-label": "selector-value",
			},
		},
		Spec: corev1.PodSpec{
			NodeName: "node",
		},
		Status: readyStatus,
	}
	readinessGatePod := corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"irrelevant-label": "irrelevant-value",
			},
		},
		Spec: corev1.PodSpec{
			NodeName: "node",
			ReadinessGates: []corev1.PodReadinessGate{
				{ConditionType: "target-health.elbv2.k8s.aws"},
			},
		},
		Status: corev1.PodStatus{
			Conditions: []corev1.PodCondition{
				{Type: corev1.PodReady, Status: corev1.ConditionTrue},
				{Type: corev1.PodConditionType("target-health.elbv2.k8s.aws"), Status: corev1.ConditionTrue},
			},
		},
	}
	unboundPod := corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"irrelevant-label": "irrelevant-value",
			},
		},
		Spec: corev1.PodSpec{
			NodeName: "node",
		},
		Status: readyStatus,
	}
	notReadyPod := corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"selector-label": "selector-value",
			},
		},
		Spec: corev1.PodSpec{
			NodeName: "node",
		},
	}
	isolatedPod := corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Labels: map[string]string{
				"pod-graceful-drain/wait": "true",
			},
			Annotations: map[string]string{
				"pod-graceful-drain/deleteAt": deleteAt,
			},
		},
		Spec: corev1.PodSpec{
			NodeName: "node",
		},
		Status: readyStatus,
	}
	nowaitPod := corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "pod",
			Annotations: map[string]string{
				"pod-graceful-drain/deleteAt": "2006-01-02T15:04:05Z",
			},
		},
		Spec: corev1.PodSpec{
			NodeName: "node",
		},
		Status: readyStatus,
	}

	service := corev1.Service{
		ObjectMeta: metav1.ObjectMeta{
			Name: "svc",
		},
		Spec: corev1.ServiceSpec{
			Selector: map[string]string{
				"selector-label": "selector-value",
			},
		},
	}

	targetTypeIP := elbv2.TargetTypeIP
	tgbIP := elbv2.TargetGroupBinding{
		ObjectMeta: metav1.ObjectMeta{
			Name: "tgb",
		},
		Spec: elbv2.TargetGroupBindingSpec{
			TargetType: &targetTypeIP,
			ServiceRef: elbv2.ServiceReference{Name: "svc"},
		},
	}
	targetTypeInstance := elbv2.TargetTypeInstance
	tgbInstance := elbv2.TargetGroupBinding{
		ObjectMeta: metav1.ObjectMeta{
			Name: "tgb",
		},
		Spec: elbv2.TargetGroupBindingSpec{
			TargetType: &targetTypeInstance,
			ServiceRef: elbv2.ServiceReference{Name: "svc"},
		},
	}

	normalNode := corev1.Node{
		ObjectMeta: metav1.ObjectMeta{
			Name: "node",
		},
	}
	unschedulableNode := corev1.Node{
		ObjectMeta: metav1.ObjectMeta{
			Name: "node",
		},
		Spec: corev1.NodeSpec{
			Unschedulable: true,
		},
	}
	taintedNode := corev1.Node{
		ObjectMeta: metav1.ObjectMeta{
			Name: "node",
		},
		Spec: corev1.NodeSpec{
			Taints: []corev1.Taint{
				{Key: corev1.TaintNodeUnschedulable},
			},
		},
	}

	tests := []struct {
		name     string
		existing []runtime.Object
		config   []PodGracefulDrainConfig
		given    *corev1.Pod
		want     *podDelayedRemoveSpec
	}{
		{
			name:     "bound pod should be delayed",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &boundPod,
			want: &podDelayedRemoveSpec{
				isolate:     true,
				asyncDelete: true,
				duration:    deleteAfter,
			},
		}, {
			name:     "bound pod should be delayed with no-deny",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{noDenyConfig},
			given:    &boundPod,
			want: &podDelayedRemoveSpec{
				isolate:     true,
				asyncDelete: false,
				duration:    admissionDelay,
			},
		},
		{
			name:     "pod with readiness gate should be delayed",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &readinessGatePod,
			want: &podDelayedRemoveSpec{
				isolate:     true,
				asyncDelete: true,
				duration:    deleteAfter,
			},
		},
		{
			name:     "pod with readiness gate should be delayed with no-deny",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{noDenyConfig},
			given:    &readinessGatePod,
			want: &podDelayedRemoveSpec{
				isolate:     true,
				asyncDelete: false,
				duration:    admissionDelay,
			},
		},
		{
			name:     "unbound pod is deleted immediately",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig, noDenyConfig},
			given:    &unboundPod,
			want:     nil,
		},
		{
			name:     "isolated pod should be delayed, again",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &isolatedPod,
			want: &podDelayedRemoveSpec{
				isolate:     false,
				asyncDelete: true,
			},
		},
		{
			name:     "isolated pod should be delayed, again with no-deny",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{noDenyConfig},
			given:    &isolatedPod,
			want: &podDelayedRemoveSpec{
				isolate:     false,
				asyncDelete: false,
				duration:    admissionDelay,
			},
		},
		{
			name:     "not ready pod should be deleted immediately",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig, noDenyConfig},
			given:    &notReadyPod,
			want:     nil,
		},
		{
			name:     "pod that deleted wait label should be deleted immediately",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig, noDenyConfig},
			given:    &nowaitPod,
			want:     nil,
		},
		{
			name:     "pod of instance type service is removed immediately",
			existing: []runtime.Object{&normalNode, &tgbInstance, &service},
			config:   []PodGracefulDrainConfig{defaultConfig, noDenyConfig},
			given:    &boundPod,
			want:     nil,
		},
		{
			name:     "pod in unschedulable node is delayed, but without async delete",
			existing: []runtime.Object{&unschedulableNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &boundPod,
			want: &podDelayedRemoveSpec{
				isolate:     true,
				asyncDelete: false,
				duration:    admissionDelay,
			},
		},
		{
			name:     "pod in tainted node is delayed, but without async delete",
			existing: []runtime.Object{&taintedNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &boundPod,
			want: &podDelayedRemoveSpec{
				isolate:     true,
				asyncDelete: false,
				duration:    admissionDelay,
			},
		},
	}

	for _, tt := range tests {
		for i, config := range tt.config {
			t.Run(fmt.Sprintf("%v - config %v", tt.name, i), func(t *testing.T) {
				ctx := context.Background()
				k8sSchema := runtime.NewScheme()
				assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
				assert.NilError(t, elbv2.AddToScheme(k8sSchema))
				builder := fake.NewClientBuilder().WithScheme(k8sSchema)
				for _, existing := range tt.existing {
					builder = builder.WithRuntimeObjects(existing.DeepCopyObject())
				}
				k8sClient := builder.WithRuntimeObjects(tt.given).Build()

				drain := NewPodGracefulDrain(k8sClient, zap.New(), &config)
				spec, err := drain.getPodDelayedRemoveSpec(ctx, tt.given.DeepCopy(), now)
				assert.NilError(t, err)
				assert.DeepEqual(t, spec, tt.want)
			})
		}
	}
}
