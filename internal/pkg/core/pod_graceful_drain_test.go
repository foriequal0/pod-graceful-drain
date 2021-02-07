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

var (
	deleteAfter    = 60 * time.Second
	contextTimeout = 10 * time.Second
)

var (
	defaultConfig = PodGracefulDrainConfig{
		DeleteAfter:     deleteAfter,
		NoDenyAdmission: false,
	}
	noDenyConfig = PodGracefulDrainConfig{
		NoDenyAdmission: true,
	}
)

func TestDelayedPodDeletionSpec(t *testing.T) {
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

	type wantedSpec struct {
		Isolate                 bool
		DeleteAt                time.Time
		AsyncDeleteTaskDuration time.Duration
		SleepTaskDuration       time.Duration
		Reason                  string
		Admission               AdmissionResponse
	}

	tests := []struct {
		name     string
		existing []runtime.Object
		config   []PodGracefulDrainConfig
		given    *corev1.Pod
		timeout  *time.Duration
		want     *wantedSpec
	}{
		{
			name:     "bound pod should be delayed",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &boundPod,
			want: &wantedSpec{
				Isolate:                 true,
				DeleteAt:                now.Add(deleteAfter),
				AsyncDeleteTaskDuration: deleteAfter,
				Reason:                  "default",
				Admission: AdmissionResponse{
					Allow:  false,
					Reason: "Pod cannot be removed immediately. It will be eventually removed after waiting for the load balancer to start",
				},
			},
		}, {
			name:     "bound pod should be delayed with no-deny",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{noDenyConfig},
			timeout:  &contextTimeout,
			given:    &boundPod,
			want: &wantedSpec{
				Isolate:           true,
				DeleteAt:          now.Add(contextTimeout - admissionDelayOverhead),
				SleepTaskDuration: contextTimeout - admissionDelayOverhead,
				Reason:            "no-deny-admission config",
				Admission: AdmissionResponse{
					Allow:  true,
					Reason: "Pod deletion is delayed enough",
				},
			},
		},
		{
			name:     "pod with readiness gate should be delayed",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &readinessGatePod,
			want: &wantedSpec{
				Isolate:                 true,
				DeleteAt:                now.Add(deleteAfter),
				AsyncDeleteTaskDuration: deleteAfter,
				Reason:                  "default",
				Admission: AdmissionResponse{
					Allow:  false,
					Reason: "Pod cannot be removed immediately. It will be eventually removed after waiting for the load balancer to start",
				},
			},
		},
		{
			name:     "pod with readiness gate should be delayed with no-deny",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{noDenyConfig},
			timeout:  &contextTimeout,
			given:    &readinessGatePod,
			want: &wantedSpec{
				Isolate:           true,
				DeleteAt:          now.Add(contextTimeout - admissionDelayOverhead),
				SleepTaskDuration: contextTimeout - admissionDelayOverhead,
				Reason:            "no-deny-admission config",
				Admission: AdmissionResponse{
					Allow:  true,
					Reason: "Pod deletion is delayed enough",
				},
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
			name:     "Isolated pod should be delayed, again",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			given:    &isolatedPod,
			want: &wantedSpec{
				Reason: "default",
				Admission: AdmissionResponse{
					Allow:  false,
					Reason: "Pod cannot be removed immediately. It will be eventually removed after waiting for the load balancer to start (reentry)",
				},
			},
		},
		{
			name:     "Isolated pod should be delayed, again with no-deny",
			existing: []runtime.Object{&normalNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{noDenyConfig},
			timeout:  &contextTimeout,
			given:    &isolatedPod,
			want: &wantedSpec{
				SleepTaskDuration: contextTimeout - admissionDelayOverhead,
				Reason:            "no-deny-admission config",
				Admission: AdmissionResponse{
					Allow:  true,
					Reason: "Pod deletion is delayed enough (reentry)",
				},
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
			timeout:  &contextTimeout,
			given:    &boundPod,
			want: &wantedSpec{
				Isolate:           true,
				DeleteAt:          now.Add(contextTimeout - admissionDelayOverhead),
				SleepTaskDuration: contextTimeout - admissionDelayOverhead,
				Reason:            "node might be draining",
				Admission: AdmissionResponse{
					Allow:  true,
					Reason: "Pod deletion is delayed enough",
				},
			},
		},
		{
			name:     "pod in tainted node is delayed, but without async delete",
			existing: []runtime.Object{&taintedNode, &tgbIP, &service},
			config:   []PodGracefulDrainConfig{defaultConfig},
			timeout:  &contextTimeout,
			given:    &boundPod,
			want: &wantedSpec{
				Isolate:           true,
				DeleteAt:          now.Add(contextTimeout - admissionDelayOverhead),
				SleepTaskDuration: contextTimeout - admissionDelayOverhead,
				Reason:            "node might be draining",
				Admission: AdmissionResponse{
					Allow:  true,
					Reason: "Pod deletion is delayed enough",
				},
			},
		},
	}

	for _, tt := range tests {
		for i, config := range tt.config {
			t.Run(fmt.Sprintf("%v - config %v", tt.name, i), func(t *testing.T) {
				ctx := context.Background()
				if tt.timeout != nil {
					var cancel context.CancelFunc
					ctx, cancel = context.WithDeadline(ctx, now.Add(*tt.timeout))
					defer cancel()
				}
				k8sSchema := runtime.NewScheme()
				assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
				assert.NilError(t, elbv2.AddToScheme(k8sSchema))
				builder := fake.NewClientBuilder().WithScheme(k8sSchema)
				for _, existing := range tt.existing {
					builder = builder.WithRuntimeObjects(existing.DeepCopyObject())
				}
				k8sClient := builder.WithRuntimeObjects(tt.given).Build()

				drain := NewPodGracefulDrain(k8sClient, zap.New(), &config)
				spec, err := drain.getDelayedPodDeletionSpec(ctx, tt.given.DeepCopy(), now)
				assert.NilError(t, err)
				var convertedSpec *wantedSpec
				if spec != nil {
					convertedSpec = &wantedSpec{
						Isolate:   spec.isolate,
						DeleteAt:  spec.deleteAt,
						Reason:    spec.reason,
						Admission: spec.admission,
					}
					if spec.asyncDeleteTask != nil {
						convertedSpec.AsyncDeleteTaskDuration = spec.asyncDeleteTask.GetDuration()
					}
					if spec.sleepTask != nil {
						convertedSpec.SleepTaskDuration = spec.sleepTask.GetDuration()
					}
				}
				assert.DeepEqual(t, convertedSpec, tt.want)
			})
		}
	}
}

func TestDelayedPodEvictionSpec(t *testing.T) {
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

	node := corev1.Node{
		ObjectMeta: metav1.ObjectMeta{
			Name: "node",
		},
	}

	type wantedSpec struct {
		Isolate                 bool
		DeleteAt                time.Time
		AsyncDeleteTaskDuration time.Duration
	}

	tests := []struct {
		name     string
		existing []runtime.Object
		given    *corev1.Pod
		timeout  *time.Duration
		want     *wantedSpec
	}{
		{
			name:     "bound pod should be delayed",
			existing: []runtime.Object{&node, &tgbIP, &service},
			given:    &boundPod,
			want: &wantedSpec{
				Isolate:                 true,
				DeleteAt:                now.Add(deleteAfter),
				AsyncDeleteTaskDuration: deleteAfter,
			},
		},
		{
			name:     "pod with readiness gate should be delayed",
			existing: []runtime.Object{&node, &tgbIP, &service},
			given:    &readinessGatePod,
			want: &wantedSpec{
				Isolate:                 true,
				DeleteAt:                now.Add(deleteAfter),
				AsyncDeleteTaskDuration: deleteAfter,
			},
		},
		{
			name:     "unbound pod is deleted immediately",
			existing: []runtime.Object{&node, &tgbIP, &service},
			given:    &unboundPod,
			want:     nil,
		},
		{
			name:     "Isolated pod should be delayed, again",
			existing: []runtime.Object{&node, &tgbIP, &service},
			given:    &isolatedPod,
			want: &wantedSpec{},
		},
		{
			name:     "not ready pod should be deleted immediately",
			existing: []runtime.Object{&node, &tgbIP, &service},
			given:    &notReadyPod,
			want:     nil,
		},
		{
			name:     "pod that deleted wait label should be deleted immediately",
			existing: []runtime.Object{&node, &tgbIP, &service},
			given:    &nowaitPod,
			want:     nil,
		},
		{
			name:     "pod of instance type service is removed immediately",
			existing: []runtime.Object{&node, &tgbInstance, &service},
			given:    &boundPod,
			want:     nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx := context.Background()
			if tt.timeout != nil {
				var cancel context.CancelFunc
				ctx, cancel = context.WithDeadline(ctx, now.Add(*tt.timeout))
				defer cancel()
			}
			k8sSchema := runtime.NewScheme()
			assert.NilError(t, clientgoscheme.AddToScheme(k8sSchema))
			assert.NilError(t, elbv2.AddToScheme(k8sSchema))
			builder := fake.NewClientBuilder().WithScheme(k8sSchema)
			for _, existing := range tt.existing {
				builder = builder.WithRuntimeObjects(existing.DeepCopyObject())
			}
			k8sClient := builder.WithRuntimeObjects(tt.given).Build()

			drain := NewPodGracefulDrain(k8sClient, zap.New(), &defaultConfig)
			spec, err := drain.getDelayedPodEvictionSpec(ctx, tt.given.DeepCopy(), now)
			assert.NilError(t, err)
			var convertedSpec *wantedSpec
			if spec != nil {
				convertedSpec = &wantedSpec{
					Isolate:   spec.isolate,
					DeleteAt:  spec.deleteAt,
				}
				if spec.asyncDeleteTask != nil {
					convertedSpec.AsyncDeleteTaskDuration = spec.asyncDeleteTask.GetDuration()
				}
			}
			assert.DeepEqual(t, convertedSpec, tt.want)
		})
	}
}
