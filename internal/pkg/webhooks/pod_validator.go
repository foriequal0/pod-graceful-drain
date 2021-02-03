/*
Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package webhooks

import (
	"context"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/interceptors"
	"github.com/go-logr/logr"
	admissionv1 "k8s.io/api/admission/v1"
	v1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/types"
	"net/http"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
)

var _ admission.DecoderInjector = &PodValidator{}
var _ admission.Handler = &PodValidator{}

type PodValidator struct {
	logger      logr.Logger
	interceptor interceptors.PodDeletionInterceptor
	config      *core.PodGracefulDrainConfig

	decoder *admission.Decoder
}

func NewPodValidator(interceptor interceptors.PodDeletionInterceptor, logger logr.Logger, config *core.PodGracefulDrainConfig) PodValidator {
	return PodValidator{
		interceptor: interceptor,
		logger:      logger.WithName("pod-validation-webhook"),
		config:      config,
	}
}

func (v *PodValidator) InjectDecoder(decoder *admission.Decoder) error {
	v.decoder = decoder
	return nil
}

func (v *PodValidator) Handle(ctx context.Context, req admission.Request) admission.Response {
	switch req.Operation {
	case admissionv1.Delete:
		return v.handleDelete(ctx, req)
	default:
		return admission.Allowed("")
	}
}

func (v *PodValidator) handleDelete(ctx context.Context, req admission.Request) admission.Response {
	pod := v1.Pod{}
	if err := v.decoder.DecodeRaw(req.OldObject, &pod); err != nil {
		return admission.Errored(http.StatusBadRequest, err)
	}

	logger := v.logger.WithValues("eviction", types.NamespacedName{Namespace: pod.Namespace, Name: pod.Name})

	handler, err := v.interceptor.Intercept(ctx, &req, &pod)
	if err != nil {
		logger.Error(err, "errored while intercepting pod deletion")
		if v.config.IgnoreError {
			return admission.Allowed("ignore error during intercepting pod deletion")
		} else {
			return admission.Errored(1, err)
		}
	} else if handler != nil {
		return handler.HandleInterceptedAdmission()
	}

	return admission.Allowed("")
}

// +kubebuilder:webhook:admissionReviewVersions=v1,webhookVersions=v1,verbs=delete,path=/validate-core-v1-pod,mutating=false,failurePolicy=ignore,sideEffects=noneOnDryRun,groups=core,resources=pods,versions=v1,name=vpod.pod-graceful-drain.io

func (v *PodValidator) SetupWebhookWithManager(mgr ctrl.Manager) error {
	mgr.GetWebhookServer().Register("/validate-core-v1-pod", &admission.Webhook{
		Handler: v,
	})
	return nil
}
